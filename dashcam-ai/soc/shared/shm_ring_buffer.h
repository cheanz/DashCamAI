#pragma once
#include <stdint.h>
#include <stddef.h>

/* POSIX shared memory ring buffer for zero-copy video frame passing
 * between media-daemon (producer) and ai-daemon (consumer). */

#define SHM_RING_NAME    "/dashcam_video_ring"
#define SHM_RING_SLOTS   8          /* number of frame slots */
#define SHM_FRAME_MAX    614400     /* 1280x480 YUV420, worst case */

typedef struct {
    uint32_t width;
    uint32_t height;
    uint32_t stride;
    uint32_t size;
    uint64_t timestamp_us;
    uint8_t  data[SHM_FRAME_MAX];
} shm_frame_t;

typedef struct {
    volatile uint32_t write_idx;
    volatile uint32_t read_idx;
    shm_frame_t       slots[SHM_RING_SLOTS];
} shm_ring_t;

/* Producer API (media-daemon) */
shm_ring_t *shm_ring_create(void);
shm_frame_t *shm_ring_get_write_slot(shm_ring_t *ring);
void         shm_ring_commit_write(shm_ring_t *ring);

/* Consumer API (ai-daemon) */
shm_ring_t  *shm_ring_open(void);
shm_frame_t *shm_ring_get_read_slot(shm_ring_t *ring);
void         shm_ring_release_read(shm_ring_t *ring);

/* Cleanup */
void shm_ring_destroy(shm_ring_t *ring);
void shm_ring_close(shm_ring_t *ring);
