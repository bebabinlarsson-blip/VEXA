#include "../include/frame.h"

int parse_frame_header(const uint8_t *bytes, size_t length, uint16_t *payload_length) {
    if (bytes == NULL || payload_length == NULL || length < 2) {
        return 0;
    }

    *payload_length = (uint16_t)(((uint16_t)bytes[0] << 8) | bytes[1]);
    return 1;
}

