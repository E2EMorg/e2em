#include "e2em.h"
#include <stdio.h>
#include <stdlib.h>
#ifdef _WIN32
#include <windows.h>
#else
#include <threads.h>
#endif
#include <time.h>
/* Reads one bounded Call JSON file. Results never echo target text. */
int main(int argc, char **argv) {
    if (argc != 2 || e2em_open(2) != 0) return 2;
    FILE *file = fopen(argv[1], "rb"); if (!file) return 3;
    uint8_t input[131073]; size_t len = fread(input, 1, sizeof(input), file); fclose(file);
    uint64_t client = e2em_open(e2em_abi_version());
    uint64_t result = e2em_call(client, input, len); if (!result) return 4;
    int status = 0;
    for (int i = 0; i < 5000 && status == 0; i++) {
        status = e2em_poll(result);
        if (!status) {
#ifdef _WIN32
            Sleep(1);
#else
            struct timespec delay = {0, 1000000}; thrd_sleep(&delay, NULL);
#endif
        }
    }
    if (status != 1) return 5;
    size_t size = e2em_result_read(result, NULL, 0);
    uint8_t *buffer = malloc(size); if (!buffer) return 6;
    if (e2em_result_read(result, buffer, size) != size) return 7;
    fwrite(buffer, 1, size, stdout); free(buffer);
    e2em_result_free(result); if (e2em_poll(result) != -1) return 8;
    e2em_close(client); e2em_close(client);
    if (e2em_call(client, input, len) != 0) return 9;
    return 0;
}
