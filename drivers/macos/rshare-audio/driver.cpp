// SPDX-License-Identifier: MIT
// AudioServerPlugIn: daemon-managed, persistent-UID network audio endpoints.
#include "../../audio-common/rshare_audio_bridge.h"
#include <CoreAudio/AudioHardware.h>
#include <CoreAudio/AudioServerPlugIn.h>
#include <CoreFoundation/CoreFoundation.h>
#include <atomic>
#include <cmath>
#include <cstring>
#include <fcntl.h>
#include <libproc.h>
#include <mach/mach_time.h>
#include <mutex>
#include <string>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>
#ifndef RSA_DRIVER_TEST
#include "broker_client.h"
#endif
namespace {
constexpr unsigned MaxDevices = 256;
constexpr AudioObjectID Plugin = kAudioObjectPlugInObject;
constexpr AudioObjectPropertySelector Command = 'rsad';
struct Device {
  std::atomic<bool> active{false};
  CFStringRef uid = nullptr, name = nullptr;
  std::atomic<RShareAudioRing *> ring{nullptr};
  std::string token;
  uid_t owner = 0;
  bool input = false;
  uint32_t channels = 0, rate = 0;
  uint64_t epoch = 0;
};
Device devices[MaxDevices];
std::mutex properties;
std::atomic<UInt32> refs{1};
AudioServerPlugInHostRef host = nullptr;
double ticks_per_second = 0;
AudioObjectID device_id(unsigned slot) { return 16 + slot * 2; }
Device *lookup(AudioObjectID id) {
  if (id < 16 || (id - 16) / 2 >= MaxDevices)
    return nullptr;
  auto &d = devices[(id - 16) / 2];
  return d.active.load(std::memory_order_acquire) ? &d : nullptr;
}
bool is_stream(AudioObjectID id) { return id >= 16 && (id - 16) % 2; }
AudioObjectPropertyAddress
address(AudioObjectPropertySelector selector,
        AudioObjectPropertyScope scope = kAudioObjectPropertyScopeGlobal) {
  return {selector, scope, kAudioObjectPropertyElementMain};
}
AudioStreamBasicDescription format(const Device &d) {
  return {(Float64)d.rate,
          kAudioFormatLinearPCM,
          kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked,
          4 * d.channels,
          1,
          4 * d.channels,
          d.channels,
          32,
          0};
}
void notify(AudioObjectID id, AudioObjectPropertySelector selector) {
  if (host) {
    auto a = address(selector);
    host->PropertiesChanged(host, id, 1, &a);
  }
}
template <class T>
OSStatus put(const T &value, UInt32 available, UInt32 *size, void *output) {
  if (!output || !size || available < sizeof(T))
    return kAudioHardwareBadPropertySizeError;
  memcpy(output, &value, sizeof(T));
  *size = sizeof(T);
  return noErr;
}
OSStatus put_string(CFStringRef string, UInt32 n, UInt32 *size, void *output) {
  if (n < sizeof(CFStringRef) || !output)
    return kAudioHardwareBadPropertySizeError;
  CFRetain(string);
  return put(string, n, size, output);
}
bool direction_matches(const Device &d, AudioObjectPropertyScope scope) {
  return scope == kAudioObjectPropertyScopeGlobal ||
         scope == (d.input ? kAudioDevicePropertyScopeInput
                           : kAudioDevicePropertyScopeOutput);
}
Boolean has(AudioServerPlugInDriverRef, AudioObjectID id, pid_t,
            const AudioObjectPropertyAddress *a) {
  if (!a)
    return false;
  if (id == Plugin) {
    switch (a->mSelector) {
    case Command:
    case kAudioObjectPropertyCustomPropertyInfoList:
    case kAudioObjectPropertyBaseClass:
    case kAudioObjectPropertyClass:
    case kAudioObjectPropertyOwner:
    case kAudioObjectPropertyName:
    case kAudioObjectPropertyManufacturer:
    case kAudioObjectPropertyOwnedObjects:
    case kAudioPlugInPropertyDeviceList:
    case kAudioPlugInPropertyTranslateUIDToDevice:
    case kAudioPlugInPropertyBundleID:
      return true;
    default:
      return false;
    }
  }
  if (!lookup(id))
    return false;
  switch (a->mSelector) {
  case kAudioObjectPropertyBaseClass:
  case kAudioObjectPropertyClass:
  case kAudioObjectPropertyOwner:
  case kAudioObjectPropertyName:
  case kAudioObjectPropertyManufacturer:
  case kAudioObjectPropertyOwnedObjects:
    return true;
  default:
    break;
  }
  if (is_stream(id)) {
    switch (a->mSelector) {
    case kAudioStreamPropertyIsActive:
    case kAudioStreamPropertyDirection:
    case kAudioStreamPropertyTerminalType:
    case kAudioStreamPropertyStartingChannel:
    case kAudioStreamPropertyLatency:
    case kAudioStreamPropertyVirtualFormat:
    case kAudioStreamPropertyPhysicalFormat:
    case kAudioStreamPropertyAvailableVirtualFormats:
    case kAudioStreamPropertyAvailablePhysicalFormats:
      return true;
    default:
      return false;
    }
  }
  switch (a->mSelector) {
  case kAudioDevicePropertyDeviceUID:
  case kAudioDevicePropertyModelUID:
  case kAudioDevicePropertyTransportType:
  case kAudioDevicePropertyRelatedDevices:
  case kAudioDevicePropertyClockDomain:
  case kAudioDevicePropertyDeviceIsAlive:
  case kAudioDevicePropertyDeviceIsRunning:
  case kAudioDevicePropertyDeviceCanBeDefaultDevice:
  case kAudioDevicePropertyDeviceCanBeDefaultSystemDevice:
  case kAudioDevicePropertyLatency:
  case kAudioDevicePropertyStreams:
  case kAudioObjectPropertyControlList:
  case kAudioDevicePropertySafetyOffset:
  case kAudioDevicePropertyNominalSampleRate:
  case kAudioDevicePropertyAvailableNominalSampleRates:
  case kAudioDevicePropertyZeroTimeStampPeriod:
  case kAudioDevicePropertyIsHidden:
  case kAudioDevicePropertyStreamConfiguration:
  case kAudioDevicePropertyPreferredChannelsForStereo:
    return true;
  default:
    return false;
  }
}
OSStatus settable(AudioServerPlugInDriverRef dr, AudioObjectID id, pid_t pid,
                  const AudioObjectPropertyAddress *a, Boolean *output) {
  if (!output)
    return kAudioHardwareIllegalOperationError;
  if (!has(dr, id, pid, a))
    return kAudioHardwareUnknownPropertyError;
  *output = id == Plugin && a->mSelector == Command;
  return noErr;
}
OSStatus data_size(AudioServerPlugInDriverRef dr, AudioObjectID id, pid_t pid,
                   const AudioObjectPropertyAddress *a, UInt32, const void *,
                   UInt32 *size) {
  if (!size || !has(dr, id, pid, a))
    return kAudioHardwareUnknownPropertyError;
  std::lock_guard<std::mutex> guard(properties);
  Device *d = lookup(id);
  switch (a->mSelector) {
  case Command:
    *size = sizeof(CFDictionaryRef);
    break;
  case kAudioObjectPropertyCustomPropertyInfoList:
    *size = sizeof(AudioServerPlugInCustomPropertyInfo);
    break;
  case kAudioObjectPropertyName:
  case kAudioObjectPropertyManufacturer:
  case kAudioPlugInPropertyBundleID:
  case kAudioDevicePropertyDeviceUID:
  case kAudioDevicePropertyModelUID:
    *size = sizeof(CFStringRef);
    break;
  case kAudioPlugInPropertyDeviceList:
  case kAudioObjectPropertyOwnedObjects:
    if (id == Plugin) {
      *size = 0;
      for (auto &v : devices)
        if (v.active.load())
          *size += sizeof(AudioObjectID);
    } else
      *size = is_stream(id) ? 0 : sizeof(AudioObjectID);
    break;
  case kAudioDevicePropertyStreams:
    *size = d && direction_matches(*d, a->mScope) ? sizeof(AudioObjectID) : 0;
    break;
  case kAudioObjectPropertyControlList:
    *size = 0;
    break;
  case kAudioDevicePropertyNominalSampleRate:
    *size = sizeof(Float64);
    break;
  case kAudioDevicePropertyAvailableNominalSampleRates:
    *size = sizeof(AudioValueRange);
    break;
  case kAudioStreamPropertyVirtualFormat:
  case kAudioStreamPropertyPhysicalFormat:
    *size = sizeof(AudioStreamBasicDescription);
    break;
  case kAudioStreamPropertyAvailableVirtualFormats:
  case kAudioStreamPropertyAvailablePhysicalFormats:
    *size = sizeof(AudioStreamRangedDescription);
    break;
  case kAudioDevicePropertyStreamConfiguration:
    *size = d && direction_matches(*d, a->mScope)
                ? sizeof(AudioBufferList)
                : offsetof(AudioBufferList, mBuffers);
    break;
  case kAudioDevicePropertyPreferredChannelsForStereo:
    *size = 2 * sizeof(UInt32);
    break;
  default:
    *size = sizeof(UInt32);
    break;
  }
  return noErr;
}
OSStatus get(AudioServerPlugInDriverRef dr, AudioObjectID id, pid_t pid,
             const AudioObjectPropertyAddress *a, UInt32 qualifier_size,
             const void *qualifier, UInt32 n, UInt32 *size, void *output) {
  if (!size || !output || !has(dr, id, pid, a))
    return kAudioHardwareUnknownPropertyError;
  std::lock_guard<std::mutex> guard(properties);
  auto *d = lookup(id);
  if (id != Plugin && !d)
    return kAudioHardwareBadObjectError;
  auto u32 = [&](UInt32 value) { return put(value, n, size, output); };
  switch (a->mSelector) {
  case kAudioObjectPropertyBaseClass:
    return u32(is_stream(id) ? kAudioObjectClassID
                             : (id == Plugin ? kAudioObjectClassID
                                             : kAudioObjectClassID));
  case kAudioObjectPropertyClass:
    return u32(id == Plugin ? kAudioPlugInClassID
                            : (is_stream(id) ? kAudioStreamClassID
                                             : kAudioDeviceClassID));
  case kAudioObjectPropertyOwner:
    return u32(id == Plugin ? kAudioObjectSystemObject
                            : (is_stream(id) ? id - 1 : Plugin));
  case kAudioObjectPropertyName:
    return put_string(d ? d->name : CFSTR("RShare Network Audio"), n, size,
                      output);
  case kAudioObjectPropertyManufacturer:
    return put_string(CFSTR("R-ShareMouse"), n, size, output);
  case kAudioPlugInPropertyBundleID:
    return put_string(CFSTR("org.rshare.audio.driver"), n, size, output);
  case kAudioDevicePropertyDeviceUID:
    return put_string(d->uid, n, size, output);
  case kAudioDevicePropertyModelUID:
    return put_string(CFSTR("org.rshare.audio.v1"), n, size, output);
  case kAudioObjectPropertyCustomPropertyInfoList: {
    AudioServerPlugInCustomPropertyInfo info = {
        Command, kAudioServerPlugInCustomPropertyDataTypeCFPropertyList,
        kAudioServerPlugInCustomPropertyDataTypeNone};
    return put(info, n, size, output);
  }
  case Command: {
    CFDictionaryRef dictionary = CFDictionaryCreate(
        nullptr, nullptr, nullptr, 0, &kCFTypeDictionaryKeyCallBacks,
        &kCFTypeDictionaryValueCallBacks);
    auto result = put(dictionary, n, size, output);
    if (result)
      CFRelease(dictionary);
    return result;
  }
  case kAudioPlugInPropertyTranslateUIDToDevice: {
    if (qualifier_size != sizeof(CFStringRef) || !qualifier)
      return kAudioHardwareBadPropertySizeError;
    CFStringRef uid = *(CFStringRef *)qualifier;
    UInt32 result = kAudioObjectUnknown;
    for (unsigned i = 0; i < MaxDevices; ++i)
      if (devices[i].active.load() && CFEqual(uid, devices[i].uid)) {
        result = device_id(i);
        break;
      }
    return u32(result);
  }
  case kAudioPlugInPropertyDeviceList:
  case kAudioObjectPropertyOwnedObjects: {
    *size = 0;
    auto values = (AudioObjectID *)output;
    if (id == Plugin) {
      for (unsigned i = 0; i < MaxDevices; ++i)
        if (devices[i].active.load() && *size + 4 <= n) {
          values[*size / 4] = device_id(i);
          *size += 4;
        }
    } else if (!is_stream(id))
      return u32(id + 1);
    return noErr;
  }
  case kAudioDevicePropertyStreams:
    if (direction_matches(*d, a->mScope))
      return u32(id + 1);
    *size = 0;
    return noErr;
  case kAudioObjectPropertyControlList:
    *size = 0;
    return noErr;
  case kAudioDevicePropertyRelatedDevices:
    return u32(id);
  case kAudioDevicePropertyTransportType:
    return u32(kAudioDeviceTransportTypeVirtual);
  case kAudioDevicePropertyClockDomain:
  case kAudioDevicePropertyLatency:
  case kAudioDevicePropertySafetyOffset:
  case kAudioDevicePropertyIsHidden:
    return u32(0);
  case kAudioDevicePropertyDeviceIsAlive:
  case kAudioDevicePropertyDeviceCanBeDefaultDevice:
  case kAudioDevicePropertyDeviceCanBeDefaultSystemDevice:
  case kAudioStreamPropertyIsActive:
  case kAudioStreamPropertyStartingChannel:
    return u32(1);
  case kAudioDevicePropertyDeviceIsRunning:
    return u32(d->ring.load(std::memory_order_acquire)
                   ->clients.load(std::memory_order_acquire) > 0);
  case kAudioDevicePropertyZeroTimeStampPeriod:
    return u32(16384);
  case kAudioDevicePropertyNominalSampleRate:
    return put((Float64)d->rate, n, size, output);
  case kAudioDevicePropertyAvailableNominalSampleRates: {
    AudioValueRange range = {(Float64)d->rate, (Float64)d->rate};
    return put(range, n, size, output);
  }
  case kAudioStreamPropertyDirection:
    return u32(d->input ? 1 : 0);
  case kAudioStreamPropertyTerminalType:
    return u32(d->input ? kAudioStreamTerminalTypeMicrophone
                        : kAudioStreamTerminalTypeSpeaker);
  case kAudioStreamPropertyVirtualFormat:
  case kAudioStreamPropertyPhysicalFormat:
    return put(format(*d), n, size, output);
  case kAudioStreamPropertyAvailableVirtualFormats:
  case kAudioStreamPropertyAvailablePhysicalFormats: {
    AudioStreamRangedDescription description = {
        format(*d), {(Float64)d->rate, (Float64)d->rate}};
    return put(description, n, size, output);
  }
  case kAudioDevicePropertyStreamConfiguration: {
    AudioBufferList list = {};
    list.mNumberBuffers = direction_matches(*d, a->mScope) ? 1 : 0;
    list.mBuffers[0].mNumberChannels = d->channels;
    UInt32 needed = list.mNumberBuffers ? sizeof(list)
                                        : offsetof(AudioBufferList, mBuffers);
    if (n < needed)
      return kAudioHardwareBadPropertySizeError;
    memcpy(output, &list, needed);
    *size = needed;
    return noErr;
  }
  case kAudioDevicePropertyPreferredChannelsForStereo: {
    UInt32 channels[2] = {1, d->channels > 1 ? 2u : 1u};
    return put(channels, n, size, output);
  }
  default:
    return kAudioHardwareUnknownPropertyError;
  }
}
std::string string_value(CFDictionaryRef dict, CFStringRef key) {
  auto value = CFDictionaryGetValue(dict, key);
  if (!value || CFGetTypeID(value) != CFStringGetTypeID())
    return {};
  char text[4096];
  if (!CFStringGetCString((CFStringRef)value, text, sizeof(text),
                          kCFStringEncodingUTF8))
    return {};
  return text;
}
int64_t number(CFDictionaryRef dict, CFStringRef key) {
  auto value = CFDictionaryGetValue(dict, key);
  int64_t n = 0;
  if (value && CFGetTypeID(value) == CFNumberGetTypeID())
    CFNumberGetValue((CFNumberRef)value, kCFNumberSInt64Type, &n);
  return n;
}
OSStatus set(AudioServerPlugInDriverRef, AudioObjectID id, pid_t pid,
             const AudioObjectPropertyAddress *a, UInt32, const void *,
             UInt32 size, const void *data) {
  if (id != Plugin || !a || a->mSelector != Command)
    return kAudioHardwareUnknownPropertyError;
  if (size != sizeof(CFDictionaryRef) || !data)
    return kAudioHardwareBadPropertySizeError;
  auto dict = *(CFDictionaryRef *)data;
  if (!dict || CFGetTypeID(dict) != CFDictionaryGetTypeID())
    return kAudioHardwareIllegalOperationError;
  auto uid = string_value(dict, CFSTR("uid")),
       name = string_value(dict, CFSTR("name")),
       path = string_value(dict, CFSTR("ring"));
  if (uid.rfind("rshare-audio-", 0) != 0 || uid.size() > 128 ||
      name.size() > 1024)
    return kAudioHardwareIllegalOperationError;
  proc_bsdinfo info = {};
  if (proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, sizeof(info)) !=
      sizeof(info))
    return kAudioHardwareIllegalOperationError;
  bool remove = CFDictionaryGetValue(dict, CFSTR("remove")) == kCFBooleanTrue;
  int channels = (int)number(dict, CFSTR("channels")),
      rate = (int)number(dict, CFSTR("rate"));
  bool input = number(dict, CFSTR("input")) != 0;
  unsigned slot = MaxDevices;
  {
    std::lock_guard<std::mutex> guard(properties);
    CFStringRef wanted =
        CFStringCreateWithCString(nullptr, uid.c_str(), kCFStringEncodingUTF8);
    for (unsigned i = 0; i < MaxDevices; ++i)
      if (devices[i].uid && CFEqual(wanted, devices[i].uid)) {
        slot = i;
        break;
      }
    if (remove) {
      CFRelease(wanted);
      if (slot == MaxDevices)
        return noErr;
      auto &d = devices[slot];
      if (d.owner != info.pbi_uid)
        return kAudioHardwareIllegalOperationError;
      d.ring.load(std::memory_order_acquire)
          ->online.store(0, std::memory_order_release);
      d.active.store(false, std::memory_order_release);
    } else {
      if (channels < 1 || channels > 8 || (rate != 48000 && rate != 96000)) {
        CFRelease(wanted);
        return kAudioHardwareUnsupportedOperationError;
      }
      if (slot != MaxDevices) {
        auto &d = devices[slot];
        CFRelease(wanted);
        if (d.owner != info.pbi_uid || d.channels != (uint32_t)channels ||
            d.rate != (uint32_t)rate || d.input != input)
          return kAudioHardwareIllegalOperationError;
        if (d.token != path) {
          if (d.ring.load()->clients.load() != 0)
            return kAudioHardwareIllegalOperationError;
#ifdef RSA_DRIVER_TEST
          return kAudioHardwareIllegalOperationError;
#else
          auto replacement =
              broker_map("attach", path.c_str(), info.pbi_uid, rate, channels);
          if (!replacement)
            return kAudioHardwareIllegalOperationError;
          if (replacement->ring->channels != (uint32_t)channels ||
              replacement->ring->sample_rate != (uint32_t)rate) {
            broker_unmap(replacement);
            return kAudioHardwareIllegalOperationError;
          }
          d.ring.store(replacement->ring, std::memory_order_release);
          d.token = path;
#endif
        }
        d.active.store(true, std::memory_order_release);
      } else {
        for (unsigned i = 0; i < MaxDevices; ++i)
          if (!devices[i].uid) {
            slot = i;
            break;
          }
        if (slot == MaxDevices) {
          CFRelease(wanted);
          return kAudioHardwareIllegalOperationError;
        }
#ifdef RSA_DRIVER_TEST
        int fd = open(path.c_str(), O_RDWR | O_NOFOLLOW | O_CLOEXEC);
        struct stat stat = {};
        if (fd < 0 || fstat(fd, &stat) || !S_ISREG(stat.st_mode) ||
            stat.st_uid != info.pbi_uid || (stat.st_mode & 0077) != 0 ||
            stat.st_size != sizeof(RShareAudioRing) || stat.st_nlink != 1) {
          if (fd >= 0)
            close(fd);
          CFRelease(wanted);
          return kAudioHardwareIllegalOperationError;
        }
        auto ring =
            (RShareAudioRing *)mmap(nullptr, sizeof(RShareAudioRing),
                                    PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
        close(fd);
        if (ring == MAP_FAILED) {
          CFRelease(wanted);
          return kAudioHardwareIllegalOperationError;
        }
        if (!rsa_valid(ring) || ring->channels != (uint32_t)channels ||
            ring->sample_rate != (uint32_t)rate) {
          munmap(ring, sizeof(*ring));
          CFRelease(wanted);
          return kAudioHardwareIllegalOperationError;
        }
#else
        auto mapping =
            broker_map("attach", path.c_str(), info.pbi_uid, rate, channels);
        if (!mapping) {
          CFRelease(wanted);
          return kAudioHardwareIllegalOperationError;
        }
        auto ring = mapping->ring;
        if (ring->channels != (uint32_t)channels ||
            ring->sample_rate != (uint32_t)rate) {
          broker_unmap(mapping);
          CFRelease(wanted);
          return kAudioHardwareIllegalOperationError;
        }
// Mapping is retained for the plug-in lifetime so no realtime
// callback can observe freed pages after device removal.
#endif
        auto &d = devices[slot];
        d.uid = wanted;
        d.name = CFStringCreateWithCString(nullptr, name.c_str(),
                                           kCFStringEncodingUTF8);
        d.ring = ring;
        d.token = path;
        d.owner = info.pbi_uid;
        d.channels = channels;
        d.rate = rate;
        d.input = input;
        d.epoch = mach_absolute_time();
        d.active.store(true, std::memory_order_release);
      }
    }
  }
  notify(Plugin, kAudioPlugInPropertyDeviceList);
  notify(Plugin, kAudioObjectPropertyOwnedObjects);
  return noErr;
}
OSStatus initialize(AudioServerPlugInDriverRef, AudioServerPlugInHostRef h) {
  host = h;
  mach_timebase_info_data_t time = {};
  mach_timebase_info(&time);
  ticks_per_second = 1e9 * (double)time.denom / time.numer;
  return noErr;
}
OSStatus create(AudioServerPlugInDriverRef, CFDictionaryRef,
                const AudioServerPlugInClientInfo *, AudioObjectID *) {
  return kAudioHardwareUnsupportedOperationError;
}
OSStatus destroy(AudioServerPlugInDriverRef, AudioObjectID) {
  return kAudioHardwareUnsupportedOperationError;
}
OSStatus client(AudioServerPlugInDriverRef, AudioObjectID id,
                const AudioServerPlugInClientInfo *) {
  return lookup(id) ? noErr : kAudioHardwareBadObjectError;
}
OSStatus change(AudioServerPlugInDriverRef, AudioObjectID, UInt64, void *) {
  return kAudioHardwareUnsupportedOperationError;
}
OSStatus start(AudioServerPlugInDriverRef, AudioObjectID id, UInt32) {
  auto d = lookup(id);
  if (!d)
    return kAudioHardwareBadObjectError;
  d->ring.load(std::memory_order_acquire)
      ->clients.fetch_add(1, std::memory_order_release);
  return noErr;
}
OSStatus stop(AudioServerPlugInDriverRef, AudioObjectID id, UInt32) {
  if (id < 16 || (id - 16) / 2 >= MaxDevices)
    return kAudioHardwareBadObjectError;
  auto d = &devices[(id - 16) / 2];
  if (!d->ring)
    return kAudioHardwareBadObjectError;
  auto n = d->ring.load(std::memory_order_acquire)->clients.load();
  while (n && !d->ring.load(std::memory_order_acquire)
                   ->clients.compare_exchange_weak(n, n - 1)) {
  }
  return noErr;
}
OSStatus stamp(AudioServerPlugInDriverRef, AudioObjectID id, UInt32,
               Float64 *sample, UInt64 *ticks, UInt64 *seed) {
  auto d = lookup(id);
  if (!d || !sample || !ticks || !seed)
    return kAudioHardwareBadObjectError;
  double elapsed = (mach_absolute_time() - d->epoch) / ticks_per_second;
  double frames = std::floor(elapsed * d->rate / 16384.0) * 16384.0;
  *sample = frames;
  *ticks = d->epoch + (UInt64)(frames * ticks_per_second / d->rate);
  *seed = 1;
  return noErr;
}
OSStatus will(AudioServerPlugInDriverRef, AudioObjectID id, UInt32, UInt32 op,
              Boolean *yes, Boolean *inplace) {
  auto d = lookup(id);
  if (!d || !yes || !inplace)
    return kAudioHardwareBadObjectError;
  *yes = op == (d->input ? kAudioServerPlugInIOOperationReadInput
                         : kAudioServerPlugInIOOperationWriteMix);
  *inplace = true;
  return noErr;
}
OSStatus begin(AudioServerPlugInDriverRef, AudioObjectID, UInt32, UInt32,
               UInt32, const AudioServerPlugInIOCycleInfo *) {
  return noErr;
}
OSStatus io(AudioServerPlugInDriverRef, AudioObjectID id, AudioObjectID stream,
            UInt32, UInt32 operation, UInt32 frames,
            const AudioServerPlugInIOCycleInfo *, void *main, void *) {
  auto d = lookup(id);
  if (!d || stream != id + 1 || !main)
    return kAudioHardwareBadObjectError;
  if (frames > RSA_CAPACITY)
    return kAudioHardwareBadPropertySizeError;
  if (d->input && operation == kAudioServerPlugInIOOperationReadInput)
    rsa_read(d->ring, (float *)main, frames, d->channels);
  else if (!d->input && operation == kAudioServerPlugInIOOperationWriteMix)
    rsa_write(d->ring, (float *)main, frames, d->channels);
  return noErr;
}
HRESULT query(void *, REFIID uuid, LPVOID *result);
ULONG retain(void *) { return ++refs; }
ULONG release(void *) { return --refs; }
AudioServerPlugInDriverInterface interface = {
    nullptr, query,  retain, release, initialize, create,    destroy, client,
    client,  change, change, has,     settable,   data_size, get,     set,
    start,   stop,   stamp,  will,    begin,      io,        begin};
AudioServerPlugInDriverInterface *interface_pointer = &interface;
HRESULT query(void *, REFIID uuid, LPVOID *result) {
  if (!result)
    return E_POINTER;
  *result = nullptr;
  auto id = CFUUIDCreateFromUUIDBytes(nullptr, uuid);
  bool matches = CFEqual(id, IUnknownUUID) ||
                 CFEqual(id, kAudioServerPlugInDriverInterfaceUUID);
  CFRelease(id);
  if (!matches)
    return E_NOINTERFACE;
  *result = &interface_pointer;
  ++refs;
  return S_OK;
}
} // namespace
extern "C" __attribute__((visibility("default"))) void *
RShareAudioFactory(CFAllocatorRef, CFUUIDRef type) {
  return CFEqual(type, kAudioServerPlugInTypeUUID) ? &interface_pointer
                                                   : nullptr;
}
