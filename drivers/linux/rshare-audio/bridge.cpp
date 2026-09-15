// SPDX-License-Identifier: MIT
// One daemon-owned PipeWire stream per mapped endpoint. No shell invocation.
#include "../../audio-common/rshare_audio_bridge.h"
#include <algorithm>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fcntl.h>
#include <pipewire/pipewire.h>
#include <signal.h>
#include <spa/param/audio/format-utils.h>
#include <spa/utils/result.h>
#include <string>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>
struct Bridge {
  pw_main_loop *loop = nullptr;
  pw_stream *stream = nullptr;
  RShareAudioRing *ring = nullptr;
  bool produce = false;
  uint32_t channels = 0;
  int result = 0;
};
static void process(void *userdata) {
  auto &bridge = *(Bridge *)userdata;
  auto buffer = pw_stream_dequeue_buffer(bridge.stream);
  if (!buffer)
    return;
  auto data = buffer->buffer;
  if (data->n_datas != 1 || !data->datas[0].data || !data->datas[0].chunk) {
    pw_stream_queue_buffer(bridge.stream, buffer);
    return;
  }
  auto &plane = data->datas[0];
  auto channels = bridge.channels;
  auto stride = channels * sizeof(float);
  if (bridge.produce) {
    uint32_t frames =
        buffer->requested ? buffer->requested : plane.maxsize / stride;
    frames = std::min(frames, (uint32_t)(plane.maxsize / stride));
    if (frames > RSA_CAPACITY)
      frames = RSA_CAPACITY;
    rsa_read(bridge.ring, (float *)plane.data, frames, channels);
    plane.chunk->offset = 0;
    plane.chunk->stride = stride;
    plane.chunk->size = frames * stride;
    buffer->size = frames;
  } else if (plane.chunk->offset <= plane.maxsize &&
             plane.chunk->size <= plane.maxsize - plane.chunk->offset) {
    auto frames = plane.chunk->size / stride;
    if (frames <= RSA_CAPACITY)
      rsa_write(bridge.ring,
                (float *)((uint8_t *)plane.data + plane.chunk->offset), frames,
                channels);
  }
  pw_stream_queue_buffer(bridge.stream, buffer);
}
static void state_changed(void *userdata, pw_stream_state,
                          pw_stream_state state, const char *error) {
  auto &b = *(Bridge *)userdata;
  b.ring->clients.store(state == PW_STREAM_STATE_STREAMING ? 1 : 0,
                        std::memory_order_release);
  if (state == PW_STREAM_STATE_ERROR) {
    fprintf(stderr, "PipeWire audio stream: %s\n",
            error ? error : "unknown error");
    b.result = 1;
    pw_main_loop_quit(b.loop);
  }
}
static void shutdown(void *userdata, int) {
  pw_main_loop_quit(((Bridge *)userdata)->loop);
}
int main(int argc, char **argv) {
  if (argc == 2 && !strcmp(argv[1], "--version")) {
    puts("rshare-pipewire-bridge ABI 1");
    return 0;
  }
  if (argc != 8) {
    fprintf(stderr, "Usage: rshare-pipewire-bridge UID NAME RING input|output "
                    "RATE CHANNELS virtual|PHYSICAL_NODE_NAME\n");
    return 2;
  }
  bool input = !strcmp(argv[4], "input"),
       physical = strcmp(argv[7], "virtual") != 0;
  if (!input && strcmp(argv[4], "output")) {
    return 2;
  }
  char *end = nullptr;
  auto rate = strtoul(argv[5], &end, 10);
  if (!end || *end || (rate != 48000 && rate != 96000))
    return 2;
  auto channels = strtoul(argv[6], &end, 10);
  if (!end || *end || channels < 1 || channels > 8)
    return 2;
  int fd = open(argv[3], O_RDWR | O_CLOEXEC | O_NOFOLLOW);
  struct stat st = {};
  if (fd < 0 || fstat(fd, &st) || st.st_uid != geteuid() ||
      !S_ISREG(st.st_mode) || (st.st_mode & 0077) || st.st_nlink != 1 ||
      st.st_size != sizeof(RShareAudioRing)) {
    if (fd >= 0)
      close(fd);
    fprintf(stderr, "Unsafe or incompatible shared audio ring\n");
    return 1;
  }
  auto ring =
      (RShareAudioRing *)mmap(nullptr, sizeof(RShareAudioRing),
                              PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
  close(fd);
  if (ring == MAP_FAILED)
    return 1;
  if (!rsa_valid(ring) || ring->channels != channels ||
      ring->sample_rate != rate) {
    munmap(ring, sizeof(*ring));
    return 1;
  }
  pw_init(&argc, &argv);
  Bridge bridge;
  bridge.ring = ring;
  bridge.channels = channels;
  bridge.produce = physical ? !input : input;
  bridge.loop = pw_main_loop_new(nullptr);
  if (!bridge.loop) {
    munmap(ring, sizeof(*ring));
    pw_deinit();
    return 1;
  }
  auto props = pw_properties_new(
      PW_KEY_NODE_NAME, argv[1], PW_KEY_NODE_DESCRIPTION, argv[2],
      PW_KEY_MEDIA_TYPE, "Audio", PW_KEY_MEDIA_CATEGORY,
      bridge.produce ? "Playback" : "Capture", PW_KEY_NODE_VIRTUAL,
      physical ? "false" : "true", PW_KEY_NODE_PAUSE_ON_IDLE, "true", nullptr);
  if (!physical)
    pw_properties_set(props, PW_KEY_MEDIA_CLASS,
                      input ? "Audio/Source" : "Audio/Sink");
  else
    pw_properties_set(props, PW_KEY_TARGET_OBJECT, argv[7]);
  std::string latency =
      std::to_string(rate / 1000) + "/" + std::to_string(rate);
  pw_properties_set(props, PW_KEY_NODE_LATENCY, latency.c_str());
  pw_stream_events events = {};
  events.version = PW_VERSION_STREAM_EVENTS;
  events.state_changed = state_changed;
  events.process = process;
  bridge.stream = pw_stream_new_simple(pw_main_loop_get_loop(bridge.loop),
                                       "RShare Audio", props, &events, &bridge);
  if (!bridge.stream) {
    pw_main_loop_destroy(bridge.loop);
    munmap(ring, sizeof(*ring));
    pw_deinit();
    return 1;
  }
  pw_loop_add_signal(pw_main_loop_get_loop(bridge.loop), SIGINT, shutdown,
                     &bridge);
  pw_loop_add_signal(pw_main_loop_get_loop(bridge.loop), SIGTERM, shutdown,
                     &bridge);
  uint8_t buffer[1024];
  spa_pod_builder builder = SPA_POD_BUILDER_INIT(buffer, sizeof(buffer));
  spa_audio_info_raw info = {};
  info.format = SPA_AUDIO_FORMAT_F32;
  info.rate = rate;
  info.channels = channels;
  const uint32_t positions[8] = {SPA_AUDIO_CHANNEL_FL, SPA_AUDIO_CHANNEL_FR,
                                 SPA_AUDIO_CHANNEL_FC, SPA_AUDIO_CHANNEL_LFE,
                                 SPA_AUDIO_CHANNEL_RL, SPA_AUDIO_CHANNEL_RR,
                                 SPA_AUDIO_CHANNEL_SL, SPA_AUDIO_CHANNEL_SR};
  for (uint32_t i = 0; i < channels; ++i)
    info.position[i] = channels == 1 ? SPA_AUDIO_CHANNEL_MONO : positions[i];
  const spa_pod *params[1] = {
      spa_format_audio_raw_build(&builder, SPA_PARAM_EnumFormat, &info)};
  auto flags =
      (pw_stream_flags)(PW_STREAM_FLAG_MAP_BUFFERS | PW_STREAM_FLAG_RT_PROCESS |
                        (physical ? PW_STREAM_FLAG_AUTOCONNECT : 0));
  int result = pw_stream_connect(
      bridge.stream, bridge.produce ? PW_DIRECTION_OUTPUT : PW_DIRECTION_INPUT,
      PW_ID_ANY, flags, params, 1);
  if (result < 0) {
    fprintf(stderr, "PipeWire connect: %s\n", spa_strerror(result));
    bridge.result = 1;
  } else
    pw_main_loop_run(bridge.loop);
  ring->clients.store(0, std::memory_order_release);
  pw_stream_destroy(bridge.stream);
  pw_main_loop_destroy(bridge.loop);
  munmap(ring, sizeof(*ring));
  pw_deinit();
  return bridge.result;
}
