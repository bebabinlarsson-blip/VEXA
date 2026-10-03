#ifndef VEXA_BENCH_FRAME_H
#define VEXA_BENCH_FRAME_H

#include <stddef.h>
#include <stdint.h>

int parse_frame_header(const uint8_t *bytes, size_t length, uint16_t *payload_length);

#endif

