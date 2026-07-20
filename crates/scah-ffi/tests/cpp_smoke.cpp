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

    // Missing versus explicitly empty attribute values.
    {
        ScahQueryBuilder *attr_builder = nullptr;
        ScahQuery *attr_query = nullptr;
        ScahStore *attr_store = nullptr;
        ScahElementList *attr_list = nullptr;
        ScahElement *input = nullptr;
        uint8_t found = 0;

        expect_ok(scah_query_all(sv("input"), scah_save_all(), &attr_builder, &err), err, "attr_all");
        expect_ok(scah_query_builder_build(attr_builder, &attr_query, &err), err, "attr_build");
        scah_query_builder_free(attr_builder);

        const ScahQuery *attr_queries[1] = {attr_query};
        expect_ok(scah_parse(sv("<input disabled value=\"\">"), attr_queries, 1, &attr_store, &err),
                  err, "attr_parse");
        scah_query_free(attr_query);

        expect_ok(scah_store_get(attr_store, sv("input"), &attr_list, &found, &err), err, "attr_get");
        expect_ok(scah_element_list_get(attr_list, 0, &input, &err), err, "attr_el");
        scah_element_list_free(attr_list);

        size_t n = 0;
        expect_ok(scah_element_attribute_count(input, &n, &err), err, "attr_count");
        if (n != 2) {
            std::fprintf(stderr, "expected 2 attrs\n");
            return 1;
        }

        bool saw_disabled = false;
        bool saw_value = false;
        for (size_t i = 0; i < n; i++) {
            ScahStringView key{};
            ScahOptionalStringView value{};
            expect_ok(scah_element_attribute_at(input, i, &key, &value, &err), err, "attr_at");
            if (key.len == 8 && std::memcmp(key.data, "disabled", 8) == 0) {
                if (value.is_some != 0) {
                    std::fprintf(stderr, "disabled should be missing value\n");
                    return 1;
                }
                saw_disabled = true;
            } else if (key.len == 5 && std::memcmp(key.data, "value", 5) == 0) {
                if (value.is_some == 0 || value.value.len != 0) {
                    std::fprintf(stderr, "value should be explicitly empty\n");
                    return 1;
                }
                saw_value = true;
            }
        }
        if (!saw_disabled || !saw_value) {
            std::fprintf(stderr, "missing expected attrs\n");
            return 1;
        }

        scah_element_free(input);
        scah_store_free(attr_store);
    }

    std::puts("cpp_smoke ok");
    return 0;
}
