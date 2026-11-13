#include "city.h"
#include <cstddef>
#include <cstdint>

extern "C" std::uint64_t ysm_city_hash64_with_seed(
    const std::uint8_t* data, std::size_t len, std::uint64_t seed) {
    static const std::uint8_t empty = 0;
    const char* bytes = len == 0
        ? reinterpret_cast<const char*>(&empty)
        : reinterpret_cast<const char*>(data);
    return CityHash64WithSeed(bytes, len, seed);
}
