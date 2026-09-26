#ifndef PSFG_DIRECTOR_H
#define PSFG_DIRECTOR_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define PSFG_DIRECTOR_OK 0
#define PSFG_DIRECTOR_INVALID_BUFFER -1
#define PSFG_DIRECTOR_BUFFER_TOO_SMALL -2
#define PSFG_DIRECTOR_INTERNAL_ERROR -3
#define PSFG_DIRECTOR_INPUT_TOO_LARGE -4
#define PSFG_DIRECTOR_MAX_REQUEST_BYTES 262144

uint32_t psfg_director_abi_version(void);

/* All buffers are caller-owned, nonoverlapping, and valid for this call.
 * output_length is mandatory. Query size with output=NULL, capacity=0.
 * BUFFER_TOO_SMALL sets output_length but writes no output bytes.
 * Successful output is UTF-8 JSON without a NUL terminator.
 * OK means a response was written; inspect its status before using a decision.
 * Nonzero statuses never authorize work. No input/output pointers are retained.
 */
int32_t psfg_director_evaluate(const uint8_t *input, size_t input_length,
                             uint8_t *output, size_t output_capacity,
                             size_t *output_length);

#ifdef __cplusplus
}
#endif

#endif
