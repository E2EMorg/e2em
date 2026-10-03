#include "e2em.h"
#include <chrono>
#include <iostream>
#include <fstream>
#include <iterator>
#include <string>
#include <thread>
#include <vector>
int main(int argc, char **argv) {
    std::string call = R"({"call_id":"c","api_version":"0.1","operation":{"op":"capabilities"}})";
    if (argc == 2) {
        std::ifstream file(argv[1], std::ios::binary);
        if (!file) return 2;
        call.assign(std::istreambuf_iterator<char>(file), std::istreambuf_iterator<char>());
    }
    if (call.empty() || call.size() > 131072) return 3;
    const auto client = e2em_open(e2em_abi_version());
    const auto result = e2em_call(client, reinterpret_cast<const uint8_t*>(call.data()), call.size());
    if (!result) return 4;
    int status = 0;
    for (int i = 0; i < 5000 && status == 0; i++) {
        status = e2em_poll(result);
        if (!status) std::this_thread::sleep_for(std::chrono::milliseconds(1));
    }
    if (status != 1) return 5;
    std::vector<uint8_t> bytes(e2em_result_read(result, nullptr, 0));
    e2em_result_read(result, bytes.data(), bytes.size());
    std::cout.write(reinterpret_cast<const char*>(bytes.data()), bytes.size());
    e2em_result_free(result); e2em_close(client);
}
