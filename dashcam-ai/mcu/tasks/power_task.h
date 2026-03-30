#pragma once
#include "FreeRTOS.h"

typedef enum {
    POWER_STATE_DRIVING = 0,
    POWER_STATE_PARKED,
    POWER_STATE_PARKED_ALERT,
} power_state_t;

/**
 * power_task — FreeRTOS task (highest priority)
 *
 * Owns the driving/parked state machine. Consumes wake events from
 * wake_event_queue. Drives GPIO to wake the SoC. Notifies kws_task
 * to enable/disable KWS based on state transitions.
 *
 * Debounce: 60 seconds of inactivity required before DRIVING → PARKED
 * to prevent thrashing in stop-and-go traffic.
 */
void power_task(void *pvParameters);
