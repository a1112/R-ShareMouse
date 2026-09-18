// SPDX-License-Identifier: MIT
// Fixed v1 ABI shared by daemon workers and native audio callbacks.
#pragma once
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
#include <atomic>
#define RSA_ATOMIC(T) std::atomic<T>
#else
#include <stdatomic.h>
#define RSA_ATOMIC(T) _Atomic(T)
#endif
#define RSA_MAGIC 0x52534142u
#define RSA_ABI 1u
#define RSA_CAPACITY 4096u
#define RSA_MAX_CHANNELS 8u
struct RShareAudioRing {
  uint32_t magic, version, channels, sample_rate;
  RSA_ATOMIC(uint64_t) write_frame;
  RSA_ATOMIC(uint64_t) read_frame;
  RSA_ATOMIC(uint64_t) generation;
  RSA_ATOMIC(uint32_t) clients;
  RSA_ATOMIC(uint32_t) online;
  RSA_ATOMIC(uint64_t) underruns;
  RSA_ATOMIC(uint64_t) overruns;
  uint8_t reserved[64];
  float samples[RSA_CAPACITY * RSA_MAX_CHANNELS];
};
#ifdef __cplusplus
static_assert(offsetof(RShareAudioRing, samples) == 128, "bridge header ABI");
static_assert(sizeof(RShareAudioRing) == 131200, "bridge size ABI");
static_assert(std::atomic<uint64_t>::is_always_lock_free,
              "64-bit atomics required");
static inline bool rsa_valid(const RShareAudioRing *r) {
  return r && r->magic == RSA_MAGIC && r->version == RSA_ABI &&
         r->channels > 0 && r->channels <= 8 &&
         (r->sample_rate == 48000 || r->sample_rate == 96000);
}
// Exactly one writer and one reader per ring. A full ring drops new frames;
// neither side modifies the opposite side's cursor.
static inline uint32_t rsa_write(RShareAudioRing *r, const float *data,
                                 uint32_t frames, uint32_t channels = 0) {
  if (!r)
    return 0;
  if (channels == 0)
    channels = r->channels;
  if (channels < 1 || channels > 8 ||
      !r->online.load(std::memory_order_acquire))
    return 0;
  auto w = r->write_frame.load(std::memory_order_relaxed),
       q = r->read_frame.load(std::memory_order_acquire);
  if (w - q > RSA_CAPACITY || frames > RSA_CAPACITY - (w - q)) {
    r->overruns.fetch_add(1, std::memory_order_relaxed);
    return 0;
  }
  for (uint32_t f = 0; f < frames; ++f)
    for (uint32_t c = 0; c < channels; ++c)
      r->samples[((w + f) % RSA_CAPACITY) * channels + c] =
          data[f * channels + c];
  r->write_frame.store(w + frames, std::memory_order_release);
  return frames;
}
static inline uint32_t rsa_read(RShareAudioRing *r, float *data,
                                uint32_t frames, uint32_t channels = 0) {
  if (!r)
    return 0;
  if (channels == 0)
    channels = r->channels;
  if (channels < 1 || channels > 8)
    return 0;
  for (uint32_t f = 0; f < frames; ++f)
    for (uint32_t c = 0; c < channels; ++c)
      data[f * channels + c] = 0.0f;
  auto q = r->read_frame.load(std::memory_order_relaxed),
       w = r->write_frame.load(std::memory_order_acquire);
  if (w - q > RSA_CAPACITY)
    return 0;
  uint32_t count = (uint32_t)(w - q);
  if (count > frames)
    count = frames;
  bool online = r->online.load(std::memory_order_acquire) != 0;
  for (uint32_t f = 0; f < frames; ++f)
    for (uint32_t c = 0; c < channels; ++c)
      data[f * channels + c] =
          (online && f < count)
              ? r->samples[((q + f) % RSA_CAPACITY) * channels + c]
              : 0.0f;
  r->read_frame.store(q + count, std::memory_order_release);
  if (online && count < frames)
    r->underruns.fetch_add(1, std::memory_order_relaxed);
  return count;
}
#endif
