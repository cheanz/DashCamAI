#include <stdio.h>
#include <pthread.h>
#include "../shared/event_bus.h"
#include "../shared/shm_ring_buffer.h"
#include "capture.h"
#include "encoder.h"
#include "loop_writer.h"
#include "vad.h"

/* media-daemon — owns camera and microphone
 *
 * Threads:
 *   capture_thread   : V4L2 → encode → shm ring (video) + loop writer (eMMC)
 *   audio_thread     : ALSA → VAD → audio chunk push to ai-daemon socket
 *   event_bus_thread : receives EVT_COLLISION_DETECTED → tags pre-roll buffer
 */

static void on_collision_detected(const dashcam_event_t *evt, void *ctx) {
    loop_writer_tag_preroll(evt->data.collision.clip_id);
}

int main(void) {
    shm_ring_t *ring = shm_ring_create();
    int bus_fd = event_bus_connect();
    event_bus_subscribe(bus_fd, EVT_COLLISION_DETECTED, on_collision_detected, NULL);

    /* TODO: start capture_thread(ring) */
    /* TODO: start audio_thread()       */
    /* TODO: start event_bus thread     */

    event_bus_dispatch(bus_fd);   /* blocks */

    shm_ring_destroy(ring);
    event_bus_disconnect(bus_fd);
    return 0;
}
