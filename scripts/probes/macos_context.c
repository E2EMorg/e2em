/* Qualification-only context probe: private IPC and an embedded warning. */
#include "e2em.h"
#include "e2em_probe_request.h"
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>
#ifdef __APPLE__
/* Diagnostic SPI only; no shared-service entitlement/bypass is inferred. */
extern int sandbox_check(pid_t pid, const char *operation, int type, ...);
#endif
static int embedded_warning(void) {
    uint64_t client = e2em_open(e2em_abi_version());
    if (!client) return 0;
    uint64_t result = e2em_call(client, call_json, sizeof(call_json) - 1);
    int ready = 0;
    for (int i = 0; result && i < 5000 && ready == 0; ++i) {
        ready = e2em_poll(result);
        if (!ready) usleep(1000);
    }
    int warned = 0;
    if (ready == 1) {
        size_t size = e2em_result_read(result, NULL, 0);
        if (size > 0 && size <= 131072) {
            uint8_t *value = calloc(size + 1, 1);
            if (value && e2em_result_read(result, value, size) == size)
                warned = strstr((char *)value, "\"status\":\"assessed\"") && strstr((char *)value, "\"action\":\"warn\"");
            free(value);
        }
    }
    if (result) e2em_result_free(result);
    e2em_close(client);
    return warned;
}
int main(int argc, char **argv) {
    struct sockaddr_un address = {0};
    if (argc != 2 || strlen(argv[1]) >= sizeof(address.sun_path)) return 2;
    address.sun_family = AF_UNIX;
#ifdef __APPLE__
    address.sun_len = sizeof(address);
    int sandboxed = sandbox_check(getpid(), NULL, 0);
    if (sandboxed < 0) return 3;
#else
    int sandboxed = 0;
#endif
    strcpy(address.sun_path, argv[1]);
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    int connected = fd >= 0 && connect(fd, (struct sockaddr *)&address, sizeof(address)) == 0;
    int failure = connected ? 0 : errno;
    if (fd >= 0) close(fd);
    int fallback = embedded_warning();
    printf("{\"sandboxed\":%s,\"connected\":%s,\"connect_errno\":%d,\"embedded_warning\":%s}\n",
           sandboxed ? "true" : "false", connected ? "true" : "false", failure, fallback ? "true" : "false");
    return fallback ? 0 : 4;
}
