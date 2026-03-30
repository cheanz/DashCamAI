#pragma once
#include "FreeRTOS.h"
#include "queue.h"

/* Wake event types posted to wake_event_queue */
typedef enum {
    WAKE_REASON_GSENSOR  = 0,
    WAKE_REASON_KWS      = 1,
} wake_reason_t;

typedef struct {
    wake_reason_t reason;
    uint32_t      timestamp_ms;
} wake_event_t;

extern QueueHandle_t wake_event_queue;

/**
 * gsensor_task — FreeRTOS task
 *
 * Polls / services G-sensor interrupt. Applies configurable impact threshold
 * filter. On breach, enqueues a WAKE_REASON_GSENSOR event.
 *
 * Active in both DRIVING and PARKED states. In DRIVING state the event
 * triggers collision recording; in PARKED state it wakes the SoC.
 */
void gsensor_task(void *pvParameters);
