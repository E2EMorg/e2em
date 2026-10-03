#ifndef E2EM_H
#define E2EM_H
#include <stdint.h>
#include <stddef.h>
#ifdef __cplusplus
extern "C" {
#endif
uint32_t e2em_abi_version(void);
uint64_t e2em_open(uint32_t abi);
/* Open a provisioned native model store. Returns zero on invalid config. */
uint64_t e2em_open_models(uint32_t abi, const uint8_t *config_path, size_t len);
uint64_t e2em_call(uint64_t client, const uint8_t *input, size_t len);
int32_t e2em_poll(uint64_t result);
size_t e2em_result_read(uint64_t result, uint8_t *buffer, size_t capacity);
void e2em_result_free(uint64_t result);
void e2em_close(uint64_t client);
#ifdef __cplusplus
}
#endif
#endif
