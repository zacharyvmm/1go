/* C++ smoke test: compile scah.h as C++ and run a minimal parse. */
#include "scah.h"

#include <cstdio>
#include <cstdlib>
#include <cstring>

static ScahStringView sv(const char *s) {
    ScahStringView v;
    v.data = reinterpret_cast<const uint8_t *>(s);
    v.len = std::strlen(s);
    return v;
}

static void expect_ok(ScahStatus status, ScahError *&err, const char *ctx) {
    if (status != ScahStatus_Ok) {
        if (err != nullptr) {
            const ScahStringView msg = scah_error_message(err);

            std::fprintf(stderr, "FAIL: %s status=%d msg=%.*s\n", ctx,
                         static_cast<int>(status), static_cast<int>(msg.len),
                         reinterpret_cast<const char *>(msg.data));

            scah_error_free(err);
            err = nullptr;
        } else {
            std::fprintf(stderr, "FAIL: %s status=%d\n", ctx, static_cast<int>(status));
        }

        std::exit(1);
    }

    if (err != nullptr) {
        scah_error_free(err);
        err = nullptr;
    }
}

static void expect_status(ScahStatus actual, ScahStatus expected, ScahError *&err,
                          const char *ctx) {
    if (actual != expected) {
        std::fprintf(stderr, "FAIL: %s expected=%d actual=%d\n", ctx,
                     static_cast<int>(expected), static_cast<int>(actual));

        if (err != nullptr) {
            scah_error_free(err);
            err = nullptr;
        }

        std::exit(1);
    }

    if (err == nullptr) {
        std::fprintf(stderr, "FAIL: %s expected diagnostic error\n", ctx);
        std::exit(1);
    }

    const ScahStringView msg = scah_error_message(err);

    if (msg.data == nullptr || msg.len == 0) {
        std::fprintf(stderr, "FAIL: %s expected non-empty diagnostic\n", ctx);

        scah_error_free(err);
        err = nullptr;
        std::exit(1);
    }

    scah_error_free(err);
    err = nullptr;
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

    ScahElementList *list = nullptr;
    uint8_t found = 0;
    expect_ok(scah_store_get(store, sv("a"), &list, &found, &err), err, "store_get");
    if (!found || list == nullptr) {
        std::fprintf(stderr, "anchor not found\n");
        return 1;
    }

    const ScahElementId *ids = nullptr;
    size_t id_len = 0;
    expect_ok(scah_element_list_ids(list, &ids, &id_len, &err), err, "ids");
    if (ids == nullptr || id_len == 0) {
        std::fprintf(stderr, "missing ids\n");
        return 1;
    }

    ScahStringView name{};
    expect_ok(scah_element_name(list, ids[0], &name, &err), err, "name");
    if (name.len != 1 || name.data[0] != 'a') {
        std::fprintf(stderr, "unexpected name\n");
        return 1;
    }

    scah_query_builder_free(builder);
    scah_query_free(query);
    scah_store_free(store);
    scah_element_list_free(list);

    // Missing versus explicitly empty attribute values.
    {
        ScahQueryBuilder *attr_builder = nullptr;
        ScahQuery *attr_query = nullptr;
        ScahStore *attr_store = nullptr;
        ScahElementList *attr_list = nullptr;
        uint8_t attr_found = 0;

        expect_ok(scah_query_all(sv("input"), scah_save_all(), &attr_builder, &err), err,
                  "attr_all");
        expect_ok(scah_query_builder_build(attr_builder, &attr_query, &err), err, "attr_build");
        scah_query_builder_free(attr_builder);

        const ScahQuery *attr_queries[1] = {attr_query};
        expect_ok(scah_parse(sv("<input disabled value=\"\">"), attr_queries, 1, &attr_store, &err),
                  err, "attr_parse");
        scah_query_free(attr_query);

        expect_ok(scah_store_get(attr_store, sv("input"), &attr_list, &attr_found, &err), err,
                  "attr_get");
        const ScahElementId *attr_ids = nullptr;
        size_t attr_id_len = 0;
        expect_ok(scah_element_list_ids(attr_list, &attr_ids, &attr_id_len, &err), err,
                  "attr_ids");
        ScahElementId input = attr_ids[0];

        size_t n = 0;
        expect_ok(scah_element_attribute_count(attr_list, input, &n, &err), err, "attr_count");
        if (n != 2) {
            std::fprintf(stderr, "expected 2 attrs\n");
            return 1;
        }

        bool saw_disabled = false;
        bool saw_value = false;
        for (size_t i = 0; i < n; i++) {
            ScahStringView key{};
            ScahOptionalStringView value{};
            expect_ok(scah_element_attribute_at(attr_list, input, i, &key, &value, &err), err,
                      "attr_at");
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

        scah_element_list_free(attr_list);
        scah_store_free(attr_store);
    }

    // Invalid selector: build must fail with a diagnostic and null query.
    {
        ScahQueryBuilder *invalid_builder = nullptr;
        ScahQuery *invalid_query = nullptr;

        expect_ok(scah_query_all(sv(""), scah_save_all(), &invalid_builder, &err), err,
                  "invalid_builder_create");

        const ScahStatus invalid_status =
            scah_query_builder_build(invalid_builder, &invalid_query, &err);

        expect_status(invalid_status, ScahStatus_InvalidSelector, err, "invalid_selector_build");

        if (invalid_query != nullptr) {
            std::fprintf(stderr, "invalid build unexpectedly produced query\n");
            return 1;
        }

        scah_query_builder_free(invalid_builder);
    }

    std::puts("cpp_smoke ok");
    return 0;
}
