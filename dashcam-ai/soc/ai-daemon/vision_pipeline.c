#include "vision_pipeline.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include "rknn_api.h"   /* Rockchip RKNN SDK */

/* ── Internal state ────────────────────────────────────────────────────── */

static rknn_context  g_ctx       = 0;
static int           g_model_w   = 416;   /* YOLO-nano input width  */
static int           g_model_h   = 416;   /* YOLO-nano input height */
static int           g_n_classes = 3;     /* vehicle, pedestrian, cyclist */

/* ── NMS helpers ───────────────────────────────────────────────────────── */

static float iou(const detection_t *a, const detection_t *b) {
    float ix1 = fmaxf(a->x1, b->x1);
    float iy1 = fmaxf(a->y1, b->y1);
    float ix2 = fminf(a->x2, b->x2);
    float iy2 = fminf(a->y2, b->y2);
    float inter = fmaxf(0, ix2 - ix1) * fmaxf(0, iy2 - iy1);
    float area_a = (a->x2 - a->x1) * (a->y2 - a->y1);
    float area_b = (b->x2 - b->x1) * (b->y2 - b->y1);
    return inter / (area_a + area_b - inter + 1e-6f);
}

static void nms(detection_t *dets, int *count, float iou_thresh) {
    for (int i = 0; i < *count; i++) {
        if (dets[i].confidence < 0) continue;
        for (int j = i + 1; j < *count; j++) {
            if (dets[j].class_id == dets[i].class_id &&
                iou(&dets[i], &dets[j]) > iou_thresh) {
                if (dets[j].confidence > dets[i].confidence) {
                    dets[i].confidence = -1;   /* suppress i */
                    break;
                } else {
                    dets[j].confidence = -1;   /* suppress j */
                }
            }
        }
    }
    /* Compact — remove suppressed entries */
    int out = 0;
    for (int i = 0; i < *count; i++) {
        if (dets[i].confidence >= 0) dets[out++] = dets[i];
    }
    *count = out;
}

/* ── Public API ────────────────────────────────────────────────────────── */

int vision_pipeline_init(const char *model_path) {
    int ret = rknn_init(&g_ctx, (void *)model_path, 0, 0, NULL);
    if (ret < 0) {
        fprintf(stderr, "[vision] rknn_init failed: %d\n", ret);
        return -1;
    }

    /* Query input dimensions from model */
    rknn_input_output_num io_num;
    rknn_query(g_ctx, RKNN_QUERY_IN_OUT_NUM, &io_num, sizeof(io_num));

    rknn_tensor_attr input_attr = {0};
    input_attr.index = 0;
    rknn_query(g_ctx, RKNN_QUERY_INPUT_ATTR, &input_attr, sizeof(input_attr));
    g_model_w = input_attr.dims[2];
    g_model_h = input_attr.dims[1];

    fprintf(stderr, "[vision] loaded %s  input=%dx%d\n",
            model_path, g_model_w, g_model_h);
    return 0;
}

int vision_pipeline_run(shm_ring_t *ring, vision_result_t *result) {
    shm_frame_t *frame = shm_ring_get_read_slot(ring);
    if (!frame) return -1;

    result->frame_timestamp_us = frame->timestamp_us;
    result->count = 0;

    /* Resize frame to model input size and convert colorspace if needed */
    /* TODO: use RGA (Rockchip Graphics Acceleration) for zero-copy resize */
    uint8_t *input_buf = malloc(g_model_w * g_model_h * 3);
    if (!input_buf) { shm_ring_release_read(ring); return -1; }

    /* TODO: rga_resize(frame->data, frame->width, frame->height,
                        input_buf, g_model_w, g_model_h, RGA_FORMAT_RGB_888); */

    /* Set RKNN input */
    rknn_input inputs[1] = {{
        .index  = 0,
        .buf    = input_buf,
        .size   = g_model_w * g_model_h * 3,
        .pass_through = 0,
        .type   = RKNN_TENSOR_UINT8,
        .fmt    = RKNN_TENSOR_NHWC,
    }};
    rknn_inputs_set(g_ctx, 1, inputs);

    /* Run inference */
    int ret = rknn_run(g_ctx, NULL);
    shm_ring_release_read(ring);
    free(input_buf);
    if (ret < 0) return -1;

    /* Get outputs — YOLO-nano produces 3 feature map outputs */
    rknn_output outputs[3] = {{.want_float = 1}, {.want_float = 1}, {.want_float = 1}};
    ret = rknn_outputs_get(g_ctx, 3, outputs, NULL);
    if (ret < 0) return -1;

    /* Decode YOLO output tensors into detections */
    /* TODO: implement full YOLO decode (anchors, sigmoid, grid offsets) */
    /* Pseudocode:
     * for each output tensor:
     *   for each grid cell:
     *     for each anchor:
     *       decode tx,ty,tw,th,conf,class_probs
     *       if conf * class_prob > NEAR_MISS_CONF_MIN:
     *         append to result->detections
     */

    rknn_outputs_release(g_ctx, 3, outputs);

    /* Apply NMS to remove duplicate detections */
    nms(result->detections, &result->count, 0.45f);

    return 0;
}

int vision_pipeline_classify_event(const vision_result_t *result,
                                   event_type_t *evt_out,
                                   float *confidence_out) {
    float best_conf = 0.0f;
    event_type_t best_evt = 0;

    for (int i = 0; i < result->count; i++) {
        const detection_t *d = &result->detections[i];

        if (d->confidence >= COLLISION_CONF_MIN) {
            if (d->confidence > best_conf) {
                best_conf = d->confidence;
                best_evt  = EVT_COLLISION_DETECTED;
            }
        } else if (d->confidence >= NEAR_MISS_CONF_MIN) {
            if (d->confidence > best_conf) {
                best_conf = d->confidence;
                best_evt  = EVT_OBJECT_DETECTED;
            }
        }
    }

    if (best_conf > 0.0f) {
        *evt_out        = best_evt;
        *confidence_out = best_conf;
        return 1;   /* event detected */
    }
    return 0;   /* no event */
}

void vision_pipeline_destroy(void) {
    if (g_ctx) rknn_destroy(g_ctx);
    g_ctx = 0;
}
