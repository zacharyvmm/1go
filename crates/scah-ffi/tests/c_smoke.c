/* C smoke test for the scah C ABI. */
#include "scah.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static void fail(const char *msg) {
    fprintf(stderr, "FAIL: %s\n", msg);
    exit(1);
}

static void expect_ok(ScahStatus status, ScahError *err, const char *ctx) {
    if (status != ScahStatus_Ok) {
        if (err != NULL) {
            ScahStringView msg = scah_error_message(err);
            fprintf(stderr, "FAIL: %s status=%d msg=%.*s\n", ctx, (int)status,
                    (int)msg.len, (const char *)msg.data);
            scah_error_free(err);
        } else {
            fprintf(stderr, "FAIL: %s status=%d\n", ctx, (int)status);
        }
        exit(1);
    }
    if (err != NULL) {
        scah_error_free(err);
    }
}

static ScahStringView sv(const char *s) {
    ScahStringView v;
    v.data = (const uint8_t *)s;
    v.len = strlen(s);
    return v;
}

int main(void) {
    if (scah_abi_version() != 1) {
        fail("unexpected abi version");
    }

    ScahError *err = NULL;
    ScahQueryBuilder *root = NULL;
    ScahQueryBuilder *branch_a = NULL;
    ScahQueryBuilder *branch_b = NULL;
    ScahQuery *query = NULL;
    ScahStore *store = NULL;
    ScahElementList *list = NULL;
    ScahElement *element = NULL;

    expect_ok(scah_query_all(sv("div"), scah_save_all(), &root, &err), err, "query_all");
    expect_ok(scah_query_builder_all(root, sv("section"), scah_save_none(), &err), err,
              "builder_all");

    ScahQuerySectionId parent = 0;
    expect_ok(scah_query_builder_current_section(root, &parent, &err), err, "current_section");

    expect_ok(scah_query_all(sv("a"), scah_save_all(), &branch_a, &err), err, "branch_a");
    expect_ok(scah_query_first(sv("h1"), scah_save_only_text_content(), &branch_b, &err), err,
              "branch_b");

    expect_ok(scah_query_builder_append(root, parent, branch_a, &err), err, "append_a");
    expect_ok(scah_query_builder_append(root, parent, branch_b, &err), err, "append_b");

    expect_ok(scah_query_builder_build(root, &query, &err), err, "build");

    /* Child builders remain valid after append/build. */
    ScahQuery *child_query = NULL;
    expect_ok(scah_query_builder_build(branch_a, &child_query, &err), err, "build_child");
    scah_query_free(child_query);

    const char *html =
        "<div><section><a href=\"https://example.com\" class=\"link\" id=\"x\">Hi</a>"
        "<h1>Title</h1></section></div>";
    const ScahQuery *queries[1] = {query};
    expect_ok(scah_parse(sv(html), queries, 1, &store, &err), err, "parse");

    /* Free original query; store must remain valid. */
    scah_query_free(query);
    query = NULL;
    scah_query_builder_free(root);
    scah_query_builder_free(branch_a);
    scah_query_builder_free(branch_b);
    root = branch_a = branch_b = NULL;

    size_t len = 0;
    expect_ok(scah_store_len(store, &len, &err), err, "store_len");
    if (len == 0) {
        fail("expected matched elements");
    }

    uint8_t found = 0;
    expect_ok(scah_store_get(store, sv("div"), &list, &found, &err), err, "store_get");
    if (!found || list == NULL) {
        fail("div not found");
    }

    expect_ok(scah_element_list_get(list, 0, &element, &err), err, "list_get");
    scah_element_list_free(list);
    list = NULL;

    ScahStringView name;
    expect_ok(scah_element_name(element, &name, &err), err, "element_name");
    if (name.len != 3 || memcmp(name.data, "div", 3) != 0) {
        fail("unexpected name");
    }

    /* Nested lookup: section is a child of div; a is a child of section. */
    ScahElementList *sections = NULL;
    expect_ok(scah_element_get(element, sv("section"), &sections, &found, &err), err,
              "element_get_section");
    if (!found || sections == NULL) {
        fail("nested section not found");
    }

    ScahElement *section = NULL;
    expect_ok(scah_element_list_get(sections, 0, &section, &err), err, "section_get");
    scah_element_list_free(sections);

    ScahElementList *children = NULL;
    expect_ok(scah_element_get(section, sv("a"), &children, &found, &err), err, "element_get_a");
    if (!found || children == NULL) {
        fail("nested a not found");
    }

    ScahElement *anchor = NULL;
    expect_ok(scah_element_list_get(children, 0, &anchor, &err), err, "child_get");
    scah_element_list_free(children);

    /* Free store while element handles remain alive. */
    scah_store_free(store);
    store = NULL;

    ScahOptionalStringView href;
    expect_ok(scah_element_get_attribute(anchor, sv("href"), &href, &err), err, "href");
    if (!href.is_some) {
        fail("missing href");
    }

    ScahOptionalStringView text;
    expect_ok(scah_element_text_content(anchor, &text, &err), err, "text");
    if (!text.is_some) {
        fail("missing text");
    }

    size_t attr_count = 0;
    expect_ok(scah_element_attribute_count(anchor, &attr_count, &err), err, "attr_count");

    scah_element_free(anchor);
    scah_element_free(section);
    scah_element_free(element);

    /* Null-safe frees */
    scah_query_builder_free(NULL);
    scah_query_free(NULL);
    scah_store_free(NULL);
    scah_element_free(NULL);
    scah_element_list_free(NULL);
    scah_error_free(NULL);

    /* Self-append: clone-before-mutate must succeed without aliasing UB. */
    {
        ScahQueryBuilder *self_builder = NULL;
        ScahQuery *self_query = NULL;
        ScahQuerySectionId self_parent = 0;

        expect_ok(scah_query_all(sv("div"), scah_save_all(), &self_builder, &err), err,
                  "self_query_all");
        expect_ok(scah_query_builder_current_section(self_builder, &self_parent, &err), err,
                  "self_current_section");
        expect_ok(scah_query_builder_append(self_builder, self_parent, self_builder, &err), err,
                  "self_append");
        expect_ok(scah_query_builder_build(self_builder, &self_query, &err), err, "self_build");

        scah_query_free(self_query);
        scah_query_builder_free(self_builder);
    }

    puts("c_smoke ok");
    return 0;
}
