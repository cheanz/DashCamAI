#include <stdio.h>
#include <pthread.h>
#include "../shared/event_bus.h"
#include "../shared/shm_ring_buffer.h"
#include "vision_pipeline.h"
#include "stt_pipeline.h"
#include "intent_pipeline.h"
#include "kws_pipeline.h"

/* ai-daemon — owns the NPU via RKNN SDK
 *
 * Threads:
 *   vision_thread : shm ring → YOLO-nano INT8 → EVT_COLLISION_DETECTED
 *   stt_thread    : audio socket → Whisper tiny → EVT_TRANSCRIPT_READY
 *   intent_thread : transcript → ONNX intent classifier → EVT_INTENT_CLASSIFIED
 *   kws_thread    : mic stream → KWS model → EVT_WAKE_WORD_DETECTED
 *
 * Does NOT touch storage or network.
 */

int main(void) {
    shm_ring_t *ring = shm_ring_open();
    int bus_fd = event_bus_connect();

    /* TODO: initialize RKNN SDK context */
    /* TODO: load models:
     *   vision_pipeline_init("models/yolo-nano-int8.rknn");
     *   stt_pipeline_init("models/whisper-tiny.rknn");
     *   intent_pipeline_init("models/intent-classifier.onnx");
     *   kws_pipeline_init("models/kws-driving.rknn");
     */

    /* TODO: start vision_thread(ring, bus_fd)  */
    /* TODO: start stt_thread(bus_fd)           */
    /* TODO: start intent_thread(bus_fd)        */
    /* TODO: start kws_thread(bus_fd)           */

    event_bus_dispatch(bus_fd);   /* blocks */

    shm_ring_close(ring);
    event_bus_disconnect(bus_fd);
    return 0;
}
