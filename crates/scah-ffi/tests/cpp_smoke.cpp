/* C++ smoke test: compile scah.h as C++ and run a minimal parse. */
#include "scah.h"

#include <cstdio>
#include <cstring>
#include <cstdlib>

static ScahStringView sv(const char *s) {
    ScahStringView v;
    v.data = reinterpret_cast<const uint8_t *>(s);
    v.len = std::strlen(s);
    return v;
}

static void expect_ok(ScahStatus status, ScahError *err, const char *ctx) {
    if (status != ScahStatus_Ok) {
        std::fprintf(stderr, "FAIL: %s status=%d\n", ctx, static_cast<int>(status));
        scah_error_free(err);
        std::exit(1);
    }
}

int main() {
    if (scah_abi_version() != 1u) {
        std::fprintf(stderr, "bad abi\n");
        return 1;
    }

    ScahError *err = nullptr;
    ScahQueryBuilder *builder = nullptr;
    ScahQuery *query = nullptr;
    ScahStore *store = nullptr;

    expect_ok(scah_query_all(sv("a"), scah_save_all(), &builder, &err), err, "all");
    expect_ok(scah_query_builder_build(builder, &query, &err), err, "build");

    const ScahQuery *queries[1] = {query};
    expect_ok(scah_parse(sv("<a href='x'>hi</a>"), queries, 1, &store, &err), err, "parse");

    size_t len = 0;
    expect_ok(scah_store_len(store, &len, &err), err, "len");
    if (len == 0) {
        std::fprintf(stderr, "no elements\n");
        return 1;
    }

    scah_query_builder_free(builder);
    scah_query_free(query);
    scah_store_free(store);

    std::puts("cpp_smoke ok");
    return 0;
}
