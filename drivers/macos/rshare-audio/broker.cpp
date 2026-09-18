// SPDX-License-Identifier: MIT
// launchd-owned memory broker. It does not handle audio or network packets.
#include "../../audio-common/rshare_audio_bridge.h"
#include <cctype>
#include <cerrno>
#include <cstring>
#include <dispatch/dispatch.h>
#include <map>
#include <new>
#include <pwd.h>
#include <string>
#include <sys/mman.h>
#include <unistd.h>
#include <xpc/xpc.h>
struct Entry {
  uid_t owner;
  void *region;
  xpc_object_t memory;
};
static std::map<std::string, Entry> entries;
static uid_t audio_uid = (uid_t)-1;
static void request(xpc_connection_t peer, xpc_object_t event) {
  if (xpc_get_type(event) != XPC_TYPE_DICTIONARY)
    return;
  auto reply = xpc_dictionary_create_reply(event);
  if (!reply)
    return;
  int error = EINVAL;
  auto op = xpc_dictionary_get_string(event, "operation"),
       token = xpc_dictionary_get_string(event, "token");
  uid_t caller = xpc_connection_get_euid(peer);
  if (op && token && strlen(token) == 36) {
    bool valid = true;
    for (unsigned i = 0; i < 36; ++i)
      if (!((i == 8 || i == 13 || i == 18 || i == 23)
                ? token[i] == '-'
                : isxdigit((unsigned char)token[i])))
        valid = false;
    if (valid) {
      std::string key =
          std::to_string(xpc_dictionary_get_uint64(event, "owner")) + ":" +
          token;
      auto found = entries.find(key);
      if (!strcmp(op, "create") &&
          caller == xpc_dictionary_get_uint64(event, "owner") &&
          caller != audio_uid) {
        auto rate = xpc_dictionary_get_uint64(event, "rate"),
             channels = xpc_dictionary_get_uint64(event, "channels");
        if ((rate == 48000 || rate == 96000) && channels >= 1 &&
            channels <= 8 && found == entries.end() && entries.size() < 256) {
          auto region =
              mmap(nullptr, sizeof(RShareAudioRing), PROT_READ | PROT_WRITE,
                   MAP_PRIVATE | MAP_ANON, -1, 0);
          if (region != MAP_FAILED) {
            auto ring = new (region) RShareAudioRing{};
            ring->magic = RSA_MAGIC;
            ring->version = RSA_ABI;
            ring->channels = channels;
            ring->sample_rate = rate;
            ring->generation.store(1);
            auto memory = xpc_shmem_create(region, sizeof(RShareAudioRing));
            if (memory) {
              entries.emplace(key, Entry{caller, region, memory});
              xpc_dictionary_set_value(reply, "memory", memory);
              error = 0;
            } else
              munmap(region, sizeof(RShareAudioRing));
          }
        }
      } else if (!strcmp(op, "attach") && caller == audio_uid &&
                 found != entries.end()) {
        xpc_dictionary_set_value(reply, "memory", found->second.memory);
        error = 0;
      } else if (!strcmp(op, "remove") && found != entries.end() &&
                 caller == found->second.owner) {
        ((RShareAudioRing *)found->second.region)
            ->online.store(0, std::memory_order_release);
        xpc_release(found->second.memory);
        munmap(found->second.region, sizeof(RShareAudioRing));
        entries.erase(found);
        error = 0;
      } else
        error = EACCES;
    }
  }
  xpc_dictionary_set_int64(reply, "error", error);
  xpc_connection_send_message(peer, reply);
  xpc_release(reply);
}
int main() {
  auto user = getpwnam("_coreaudiod");
  if (!user)
    return 1;
  audio_uid = user->pw_uid;
  auto queue =
      dispatch_queue_create("org.rshare.audio.broker", DISPATCH_QUEUE_SERIAL);
  auto listener = xpc_connection_create_mach_service(
      "org.rshare.audio.broker", queue, XPC_CONNECTION_MACH_SERVICE_LISTENER);
  if (!listener)
    return 1;
  xpc_connection_set_event_handler(listener, ^(xpc_object_t object) {
    if (xpc_get_type(object) != XPC_TYPE_CONNECTION)
      return;
    auto peer = (xpc_connection_t)object;
    xpc_connection_set_target_queue(peer, queue);
    xpc_connection_set_event_handler(peer, ^(xpc_object_t event) {
      request(peer, event);
    });
    xpc_connection_resume(peer);
  });
  xpc_connection_resume(listener);
  dispatch_main();
}
