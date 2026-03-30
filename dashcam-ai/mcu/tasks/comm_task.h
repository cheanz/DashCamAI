#pragma once

/**
 * comm_task — FreeRTOS task
 *
 * UART bridge between MCU and SoC. Sends state change notifications
 * (driving/parked, wake reason). Receives ACKs from SoC (e.g., "suspend ready",
 * "wake acknowledged"). Releases SoC wake GPIO after wake ACK.
 *
 * Protocol: length-prefixed binary frames (protobuf-nano recommended).
 */
void comm_task(void *pvParameters);
