// SPDX-License-Identifier: MIT
// Exercises the real plug-in vtable without installing it into coreaudiod.
#define RSA_DRIVER_TEST 1
#include "driver.cpp"
#include <cassert>
#include <cstdio>
#include <new>
int main() {
  auto driver = (AudioServerPlugInDriverRef)RShareAudioFactory(
      nullptr, kAudioServerPlugInTypeUUID);
  assert(driver);
  AudioServerPlugInHostInterface test_host = {};
  test_host.PropertiesChanged =
      [](AudioServerPlugInHostRef, AudioObjectID, UInt32,
         const AudioObjectPropertyAddress *) -> OSStatus { return noErr; };
  assert((*driver)->Initialize(driver, &test_host) == noErr);
  AudioServerPlugInIOCycleInfo cycle = {};
  AudioObjectPropertyAddress command = {Command,
                                        kAudioObjectPropertyScopeGlobal,
                                        kAudioObjectPropertyElementMain};
  char path[] = "/tmp/rshare-audio-driver-test-XXXXXX";
  int fd = mkstemp(path);
  assert(fd >= 0);
  assert(fchmod(fd, 0600) == 0);
  assert(ftruncate(fd, sizeof(RShareAudioRing)) == 0);
  auto ring =
      (RShareAudioRing *)mmap(nullptr, sizeof(RShareAudioRing),
                              PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
  assert(ring != MAP_FAILED);
  new (ring) RShareAudioRing{};
  ring->magic = RSA_MAGIC;
  ring->version = RSA_ABI;
  ring->channels = 2;
  ring->sample_rate = 48000;
  auto dict =
      CFDictionaryCreateMutable(nullptr, 0, &kCFTypeDictionaryKeyCallBacks,
                                &kCFTypeDictionaryValueCallBacks);
  CFDictionarySetValue(dict, CFSTR("uid"), CFSTR("rshare-audio-test"));
  CFDictionarySetValue(dict, CFSTR("name"), CFSTR("Test microphone"));
  auto file = CFStringCreateWithCString(nullptr, path, kCFStringEncodingUTF8);
  CFDictionarySetValue(dict, CFSTR("ring"), file);
  CFRelease(file);
  for (auto pair : {std::pair<CFStringRef, int>(CFSTR("channels"), 2),
                    {CFSTR("rate"), 48000},
                    {CFSTR("input"), 1}}) {
    auto value = CFNumberCreate(nullptr, kCFNumberIntType, &pair.second);
    CFDictionarySetValue(dict, pair.first, value);
    CFRelease(value);
  }
  assert((*driver)->SetPropertyData(driver, Plugin, getpid(), &command, 0,
                                    nullptr, sizeof(dict), &dict) == noErr);
  auto list = address(kAudioPlugInPropertyDeviceList);
  UInt32 size = 0, id = 0;
  assert((*driver)->GetPropertyDataSize(driver, Plugin, 0, &list, 0, nullptr,
                                        &size) == noErr &&
         size == 4);
  assert((*driver)->GetPropertyData(driver, Plugin, 0, &list, 0, nullptr, 4,
                                    &size, &id) == noErr &&
         id >= 16);
  assert((*driver)->StartIO(driver, id, 1) == noErr &&
         ring->clients.load() == 1);
  float output[96];
  for (auto &x : output)
    x = 1;
  assert((*driver)->DoIOOperation(driver, id, id + 1, 1,
                                  kAudioServerPlugInIOOperationReadInput, 48,
                                  &cycle, output, nullptr) == noErr);
  for (float x : output)
    assert(x == 0);
  ring->online.store(1);
  float input[96];
  for (unsigned i = 0; i < 96; ++i)
    input[i] = (float)i / 96;
  assert(rsa_write(ring, input, 48) == 48);
  assert((*driver)->DoIOOperation(driver, id, id + 1, 1,
                                  kAudioServerPlugInIOOperationReadInput, 48,
                                  &cycle, output, nullptr) == noErr);
  assert(memcmp(input, output, sizeof(input)) == 0);
  Float64 sample = 0;
  UInt64 ticks = 0, seed = 0;
  assert((*driver)->GetZeroTimeStamp(driver, id, 1, &sample, &ticks, &seed) ==
             noErr &&
         seed == 1 && ticks > 0);
  assert((*driver)->StopIO(driver, id, 1) == noErr &&
         ring->clients.load() == 0);
  CFDictionarySetValue(dict, CFSTR("remove"), kCFBooleanTrue);
  assert((*driver)->SetPropertyData(driver, Plugin, getpid(), &command, 0,
                                    nullptr, sizeof(dict), &dict) == noErr);
  assert((*driver)->GetPropertyDataSize(driver, Plugin, 0, &list, 0, nullptr,
                                        &size) == noErr &&
         size == 0);
  assert(ring->online.load() == 0);
  // A world-readable ring must never be accepted by the privileged audio host.
  CFDictionarySetValue(dict, CFSTR("uid"),
                       CFSTR("rshare-audio-bad-permissions"));
  CFDictionarySetValue(dict, CFSTR("remove"), kCFBooleanFalse);
  assert(fchmod(fd, 0644) == 0);
  assert((*driver)->SetPropertyData(driver, Plugin, getpid(), &command, 0,
                                    nullptr, sizeof(dict), &dict) != noErr);
  CFRelease(dict);
  munmap(ring, sizeof(*ring));
  close(fd);
  unlink(path);
  puts("PASS: plug-in registration, vtable properties, client lifecycle, PCM, "
       "silence, removal, permissions");
}
