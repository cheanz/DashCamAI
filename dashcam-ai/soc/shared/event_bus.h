#pragma once
#include "events.h"

/* Lightweight pub-sub event bus over Unix domain sockets.
 * Each daemon calls event_bus_subscribe() for the event types it cares about.
 * Any daemon can call event_bus_publish() to broadcast an event. */

#define EVENT_BUS_SOCKET  "/var/run/dashcam/event_bus.sock"

typedef void (*event_handler_t)(const dashcam_event_t *event, void *ctx);

/* Broker — run by a dedicated thread inside each daemon process */
int  event_bus_connect(void);
void event_bus_disconnect(int fd);

/* Subscribe to one or more event types */
int  event_bus_subscribe(int fd, event_type_t type, event_handler_t handler, void *ctx);

/* Publish an event to all subscribers */
int  event_bus_publish(int fd, const dashcam_event_t *event);

/* Blocking dispatch loop — call from a dedicated thread */
void event_bus_dispatch(int fd);
