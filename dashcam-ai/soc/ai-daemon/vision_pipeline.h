#pragma once
#include <stdint.h>
#include "../shared/shm_ring_buffer.h"
#include "../shared/events.h"

/* Maximum detections returned per frame */
#define VISION_MAX_DETECTIONS   32

/* Class IDs that trigger a collision/near-miss event */
#define CLASS_VEHICLE           0
#define CLASS_PEDESTRIAN        1
#define CLASS_CYCLIST           2

/* Confidence thresholds */
#define COLLISION_CONF_MIN      0.65f   /* high-confidence — triggers clip preservation */
#define NEAR_MISS_CONF_MIN      0.45f   /* lower confidence — logs event only */

typedef struct {
    float    x1, y1, x2, y2;   /* bounding box, normalized [0..1] */
    float    confidence;
    int      class_id;
} detection_t;

typedef struct {
    detection_t detections[VISION_MAX_DETECTIONS];
    int         count;
    uint64_t    frame_timestamp_us;
} vision_result_t;

/**
 * vision_pipeline_init — load YOLO-nano INT8 model into RKNN context.
 * @param model_path  path to .rknn model file
 * @return 0 on success, -1 on failure
 */
int  vision_pipeline_init(const char *model_path);

/**
 * vision_pipeline_run — run inference on one frame.
 * Reads the next available frame from the shm ring buffer.
 * Fills result with detected objects.
 * Caller is responsible for publishing events based on result.
 * @return 0 on success, -1 on failure
 */
int  vision_pipeline_run(shm_ring_t *ring, vision_result_t *result);

/**
 * vision_pipeline_is_collision — evaluate a result for collision/near-miss.
 * Returns INTENT_SIMPLE_COMMAND (no event), and writes event_type to
 * evt_out when a collision or near-miss is detected.
 */
int  vision_pipeline_classify_event(const vision_result_t *result,
                                    event_type_t *evt_out,
                                    float *confidence_out);

void vision_pipeline_destroy(void);
