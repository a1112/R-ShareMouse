// SPDX-License-Identifier: MIT
#pragma once
#include "../../audio-common/rshare_audio_bridge.h"
#include <dispatch/dispatch.h>
#include <memory>
#include <sys/mman.h>
#include <xpc/xpc.h>
struct BrokerMapping {
  xpc_object_t reply = nullptr;
  RShareAudioRing *ring = nullptr;
};
static BrokerMapping *broker_map(const char *operation, const char *token,
                                 uint32_t owner, uint32_t rate,
                                 uint32_t channels) {
  auto connection = xpc_connection_create_mach_service(
      "org.rshare.audio.broker", nullptr,
      XPC_CONNECTION_MACH_SERVICE_PRIVILEGED);
  if (!connection)
    return nullptr;
  xpc_connection_set_event_handler(connection, ^(xpc_object_t){
                                   });
  xpc_connection_resume(connection);
  auto request = xpc_dictionary_create(nullptr, nullptr, 0);
  xpc_dictionary_set_string(request, "operation", operation);
  xpc_dictionary_set_string(request, "token", token);
  xpc_dictionary_set_uint64(request, "owner", owner);
  xpc_dictionary_set_uint64(request, "rate", rate);
  xpc_dictionary_set_uint64(request, "channels", channels);
  // Runs on control paths only. A bounded async request avoids blocking the
  // audio server forever when an installed helper is unresponsive.
  struct ReplyState {
    dispatch_semaphore_t semaphore = dispatch_semaphore_create(0);
    xpc_object_t reply = nullptr;
    ~ReplyState() {
      if (reply)
        xpc_release(reply);
      dispatch_release(semaphore);
    }
  };
  auto state = std::make_shared<ReplyState>();
  auto queue = dispatch_queue_create("org.rshare.audio.broker.reply",
                                     DISPATCH_QUEUE_SERIAL);
  xpc_connection_send_message_with_reply(
      connection, request, queue, ^(xpc_object_t value) {
        state->reply = xpc_retain(value);
        dispatch_semaphore_signal(state->semaphore);
      });
  auto timeout = dispatch_semaphore_wait(
      state->semaphore, dispatch_time(DISPATCH_TIME_NOW, 2 * NSEC_PER_SEC));
  xpc_release(request);
  xpc_connection_cancel(connection);
  xpc_release(connection);
  dispatch_release(queue);
  // Reply state is retained by the callback after a timeout. Never wait on
  // cancellation from inside the audio host.
  if (timeout)
    return nullptr;
  auto reply = state->reply ? xpc_retain(state->reply) : nullptr;
  if (!reply || xpc_get_type(reply) != XPC_TYPE_DICTIONARY ||
      xpc_dictionary_get_int64(reply, "error") != 0) {
    if (reply)
      xpc_release(reply);
    return nullptr;
  }
  auto memory = xpc_dictionary_get_value(reply, "memory");
  void *mapped = nullptr;
  if (!memory || xpc_get_type(memory) != XPC_TYPE_SHMEM ||
      xpc_shmem_map(memory, &mapped) != sizeof(RShareAudioRing)) {
    xpc_release(reply);
    return nullptr;
  }
  auto ring = (RShareAudioRing *)mapped;
  if (!rsa_valid(ring)) {
    munmap(mapped, sizeof(*ring));
    xpc_release(reply);
    return nullptr;
  }
  return new BrokerMapping{reply, ring};
}
static void broker_unmap(BrokerMapping *mapping) {
  if (!mapping)
    return;
  munmap(mapping->ring, sizeof(RShareAudioRing));
  xpc_release(mapping->reply);
  delete mapping;
}
