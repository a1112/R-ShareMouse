// SPDX-License-Identifier: MIT
#include "../../../drivers/audio-common/rshare_audio_bridge.h"
#include <CoreAudio/CoreAudio.h>
#include <CoreFoundation/CoreFoundation.h>
#include <algorithm>
#include <cstdlib>
#include <cstring>
#include <string>
#include <sys/mman.h>
#include <unistd.h>
#include <vector>
static AudioObjectPropertyAddress
addr(AudioObjectPropertySelector s,
     AudioObjectPropertyScope scope = kAudioObjectPropertyScopeGlobal) {
  return {s, scope, kAudioObjectPropertyElementMain};
}
static std::string string_prop(AudioObjectID id,
                               AudioObjectPropertySelector sel) {
  CFStringRef value = nullptr;
  UInt32 size = sizeof(value);
  auto a = addr(sel);
  if (AudioObjectGetPropertyData(id, &a, 0, nullptr, &size, &value) != noErr ||
      !value)
    return {};
  std::vector<char> data(CFStringGetMaximumSizeForEncoding(
                             CFStringGetLength(value), kCFStringEncodingUTF8) +
                         1);
  bool ok = CFStringGetCString(value, data.data(), data.size(),
                               kCFStringEncodingUTF8);
  CFRelease(value);
  return ok ? std::string(data.data()) : std::string();
}
static std::string quoted(const std::string &s) {
  std::string out = "\"";
  const char *hex = "0123456789abcdef";
  for (unsigned char c : s) {
    if (c == '"' || c == '\\') {
      out += '\\';
      out += c;
    } else if (c < 32) {
      out += "\\u00";
      out += hex[c >> 4];
      out += hex[c & 15];
    } else
      out += c;
  }
  return out + '"';
}
extern "C" char *rsa_macos_enumerate() {
  auto a = addr(kAudioHardwarePropertyDevices);
  UInt32 size = 0;
  if (AudioObjectGetPropertyDataSize(kAudioObjectSystemObject, &a, 0, nullptr,
                                     &size) != noErr)
    return nullptr;
  std::vector<AudioObjectID> ids(size / sizeof(AudioObjectID));
  if (AudioObjectGetPropertyData(kAudioObjectSystemObject, &a, 0, nullptr,
                                 &size, ids.data()) != noErr)
    return nullptr;
  std::string json = "[";
  bool first = true;
  for (auto id : ids) {
    auto uid = string_prop(id, kAudioDevicePropertyDeviceUID),
         name = string_prop(id, kAudioObjectPropertyName);
    if (uid.empty())
      continue;
    UInt32 transport = 0, transport_size = sizeof(transport);
    auto transport_addr = addr(kAudioDevicePropertyTransportType);
    AudioObjectGetPropertyData(id, &transport_addr, 0, nullptr, &transport_size,
                               &transport);
    bool virtual_device = transport == kAudioDeviceTransportTypeVirtual ||
                          uid.rfind("rshare-audio-", 0) == 0;
    for (bool input : {true, false}) {
      auto scope = input ? kAudioDevicePropertyScopeInput
                         : kAudioDevicePropertyScopeOutput;
      auto config = addr(kAudioDevicePropertyStreamConfiguration, scope);
      UInt32 bytes = 0;
      if (AudioObjectGetPropertyDataSize(id, &config, 0, nullptr, &bytes) !=
              noErr ||
          bytes < sizeof(UInt32))
        continue;
      std::vector<uint8_t> storage(bytes);
      auto buffers = (AudioBufferList *)storage.data();
      if (AudioObjectGetPropertyData(id, &config, 0, nullptr, &bytes,
                                     buffers) != noErr)
        continue;
      unsigned channels = 0;
      for (UInt32 i = 0; i < buffers->mNumberBuffers; ++i)
        channels += buffers->mBuffers[i].mNumberChannels;
      if (!channels)
        continue;
      auto rates = addr(kAudioDevicePropertyAvailableNominalSampleRates);
      UInt32 rate_bytes = 0;
      std::vector<AudioValueRange> ranges;
      if (AudioObjectGetPropertyDataSize(id, &rates, 0, nullptr, &rate_bytes) ==
          noErr) {
        ranges.resize(rate_bytes / sizeof(AudioValueRange));
        if (AudioObjectGetPropertyData(id, &rates, 0, nullptr, &rate_bytes,
                                       ranges.data()) != noErr)
          ranges.clear();
      }
      std::string supported = "[";
      bool rate_first = true;
      for (double rate : {48000.0, 96000.0})
        if (std::any_of(ranges.begin(), ranges.end(), [rate](auto r) {
              return r.mMinimum <= rate && r.mMaximum >= rate;
            })) {
          if (!rate_first)
            supported += ",";
          supported += std::to_string((int)rate);
          rate_first = false;
        }
      supported += "]";
      if (rate_first)
        continue;
      UInt32 alive = 0;
      auto alive_addr = addr(kAudioDevicePropertyDeviceIsAlive);
      UInt32 alive_size = sizeof(alive);
      AudioObjectGetPropertyData(id, &alive_addr, 0, nullptr, &alive_size,
                                 &alive);
      if (!first)
        json += ",";
      first = false;
      json += "{\"id\":" + quoted(uid) + ",\"name\":" + quoted(name) +
              ",\"direction\":\"" + (input ? "Input" : "Output") +
              "\",\"channels\":" + std::to_string(std::min(channels, 255u)) +
              ",\"sample_rates\":" + supported +
              ",\"virtual_device\":" + (virtual_device ? "true" : "false") +
              ",\"available\":" + (alive ? "true" : "false") + "}";
    }
  }
  json += "]";
  return strdup(json.c_str());
}
extern "C" void rsa_macos_free(char *value) { free(value); }
extern "C" int32_t rsa_macos_plugin_id(uint32_t *output) {
  auto a = addr(kAudioHardwarePropertyPlugInForBundleID);
  CFStringRef bundle = CFSTR("org.rshare.audio.driver");
  UInt32 size = sizeof(*output);
  return AudioObjectGetPropertyData(kAudioObjectSystemObject, &a,
                                    sizeof(bundle), &bundle, &size, output);
}
extern "C" int32_t rsa_macos_device_command(const char *uid, const char *name,
                                            const char *ring_path,
                                            uint32_t channels, uint32_t rate,
                                            uint32_t input, bool remove) {
  uint32_t plugin = 0;
  auto status = rsa_macos_plugin_id(&plugin);
  if (status || !plugin)
    return status ? status : -1;
  auto dict =
      CFDictionaryCreateMutable(nullptr, 0, &kCFTypeDictionaryKeyCallBacks,
                                &kCFTypeDictionaryValueCallBacks);
  for (auto pair : {std::pair<const char *, const char *>("uid", uid),
                    {"name", name},
                    {"ring", ring_path}}) {
    CFStringRef key = CFStringCreateWithCString(nullptr, pair.first,
                                                kCFStringEncodingUTF8),
                value = CFStringCreateWithCString(nullptr, pair.second,
                                                  kCFStringEncodingUTF8);
    CFDictionarySetValue(dict, key, value);
    CFRelease(key);
    CFRelease(value);
  }
  for (auto pair :
       {std::pair<CFStringRef, uint32_t>(CFSTR("channels"), channels),
        {CFSTR("rate"), rate},
        {CFSTR("input"), input}}) {
    int64_t n = pair.second;
    auto value = CFNumberCreate(nullptr, kCFNumberSInt64Type, &n);
    CFDictionarySetValue(dict, pair.first, value);
    CFRelease(value);
  }
  CFDictionarySetValue(dict, CFSTR("remove"),
                       remove ? kCFBooleanTrue : kCFBooleanFalse);
  auto a = addr('rsad');
  status =
      AudioObjectSetPropertyData(plugin, &a, 0, nullptr, sizeof(dict), &dict);
  CFRelease(dict);
  return status;
}
#include "../../../drivers/macos/rshare-audio/broker_client.h"
extern "C" void *rsa_macos_bridge_create(const char *token, uint32_t rate,
                                         uint32_t channels) {
  return broker_map("create", token, geteuid(), rate, channels);
}
extern "C" RShareAudioRing *rsa_macos_bridge_ring(void *handle) {
  return handle ? ((BrokerMapping *)handle)->ring : nullptr;
}
extern "C" void rsa_macos_bridge_release(void *handle, const char *token) {
  if (!handle)
    return;
  ((BrokerMapping *)handle)->ring->online.store(0, std::memory_order_release);
  // The broker retains only live daemon-owned entries; detached HAL pages
  // remain mapped by the plug-in until its last realtime reader is gone.
  auto removed = broker_map("remove", token, geteuid(), 0, 0);
  broker_unmap(removed);
  broker_unmap((BrokerMapping *)handle);
}
