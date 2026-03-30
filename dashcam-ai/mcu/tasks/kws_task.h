#pragma once
#include "gsensor_task.h"   /* re-uses wake_event_t / wake_event_queue */

/**
 * kws_task — FreeRTOS task
 *
 * Runs a lightweight keyword-spotting model on the MCU.
 * Active only in PARKED state (gated by power_task via task notification).
 * On keyword detection, enqueues a WAKE_REASON_KWS event.
 */
void kws_task(void *pvParameters);
