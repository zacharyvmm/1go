/* Direct C ABI result-access benchmark. Emits JSON compatible with the
 * language harnesses. Link against release libscah_ffi. */

#define _POSIX_C_SOURCE 200809L

#include "scah.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

typedef struct {
  const char *name;
  double median_ns;
  double min_ns;
  double p25_ns;
  double p75_ns;
  double mad_ns;
  size_t iterations;
  size_t samples;
} SampleStats;

static double now_ns(void) {
  struct timespec ts;
  clock_gettime(CLOCK_MONOTONIC, &ts);
  return (double)ts.tv_sec * 1e9 + (double)ts.tv_nsec;
}

static int cmp_double(const void *a, const void *b) {
  double da = *(const double *)a;
  double db = *(const double *)b;
  return (da > db) - (da < db);
}

static double percentile(double *sorted, size_t n, double p) {
  if (n == 0) return 0.0;
  double k = (double)(n - 1) * p;
  size_t f = (size_t)k;
  size_t c = f + 1 < n ? f + 1 : f;
  if (f == c) return sorted[f];
  return sorted[f] + (sorted[c] - sorted[f]) * (k - (double)f);
}

static SampleStats measure(const char *name, void (*fn)(void *), void *ctx, size_t samples,
                           size_t fixed_iters, int smoke) {
  size_t iterations = fixed_iters ? fixed_iters : 1;
  if (!fixed_iters) {
    for (;;) {
      double start = now_ns();
      for (size_t i = 0; i < iterations; i++) fn(ctx);
      double elapsed = now_ns() - start;
      if (elapsed >= (smoke ? 1e7 : 1e8) || iterations >= 1000000) break;
      iterations *= 2;
    }
  }

  for (size_t w = 0; w < (samples < 3 ? samples : 3); w++) {
    for (size_t i = 0; i < iterations; i++) fn(ctx);
  }

  double *per_op = calloc(samples, sizeof(double));
  if (!per_op) abort();
  for (size_t s = 0; s < samples; s++) {
    double start = now_ns();
    for (size_t i = 0; i < iterations; i++) fn(ctx);
    per_op[s] = (now_ns() - start) / (double)iterations;
  }
  qsort(per_op, samples, sizeof(double), cmp_double);
  double med = percentile(per_op, samples, 0.5);
  double *devs = calloc(samples, sizeof(double));
  if (!devs) abort();
  for (size_t i = 0; i < samples; i++) {
    double d = per_op[i] - med;
    devs[i] = d < 0 ? -d : d;
  }
  qsort(devs, samples, sizeof(double), cmp_double);

  SampleStats out = {
      .name = name,
      .median_ns = med,
      .min_ns = per_op[0],
      .p25_ns = percentile(per_op, samples, 0.25),
      .p75_ns = percentile(per_op, samples, 0.75),
      .mad_ns = percentile(devs, samples, 0.5),
      .iterations = iterations,
      .samples = samples,
  };
  free(devs);
  free(per_op);
  return out;
}

static void die(const char *msg) {
  fprintf(stderr, "c_abi_bench: %s\n", msg);
  exit(1);
}

static ScahQuery *build_query(const char *selector) {
  ScahQueryBuilder *builder = NULL;
  ScahQuery *query = NULL;
  ScahError *err = NULL;
  ScahStringView sel = {.data = (const uint8_t *)selector, .len = strlen(selector)};
  if (scah_query_all(sel, scah_save_all(), &builder, &err) != ScahStatus_Ok) die("query_all");
  if (scah_query_builder_build(builder, &query, &err) != ScahStatus_Ok) die("build");
  scah_query_builder_free(builder);
  return query;
}

static char *make_html(size_t n) {
  /* Approximate upper bound per element. */
  size_t cap = n * 64 + 1;
  char *buf = malloc(cap);
  if (!buf) abort();
  size_t off = 0;
  for (size_t i = 0; i < n; i++) {
    int wrote = snprintf(buf + off, cap - off, "<a href=\"/%zu\" class=\"c\" id=\"i%zu\">t%zu</a>", i,
                         i, i);
    if (wrote < 0) abort();
    off += (size_t)wrote;
  }
  buf[off] = '\0';
  return buf;
}

typedef struct {
  ScahStore *store;
  ScahElementList *list;
  const ScahElementId *ids;
  size_t len;
} BenchCtx;

static void bench_store_get(void *ctx) {
  BenchCtx *b = ctx;
  ScahElementList *list = NULL;
  uint8_t found = 0;
  ScahError *err = NULL;
  ScahStringView q = {.data = (const uint8_t *)"a", .len = 1};
  if (scah_store_get(b->store, q, &list, &found, &err) != ScahStatus_Ok || !found) die("store_get");
  scah_element_list_free(list);
}

static void bench_list_ids(void *ctx) {
  BenchCtx *b = ctx;
  const ScahElementId *ids = NULL;
  size_t len = 0;
  ScahError *err = NULL;
  if (scah_element_list_ids(b->list, &ids, &len, &err) != ScahStatus_Ok) die("list_ids");
  if (len != b->len) die("len mismatch");
  volatile size_t sink = 0;
  for (size_t i = 0; i < len; i++) sink += ids[i];
  (void)sink;
}

static void bench_names(void *ctx) {
  BenchCtx *b = ctx;
  ScahError *err = NULL;
  volatile size_t sink = 0;
  for (size_t i = 0; i < b->len; i++) {
    ScahStringView name = {0};
    if (scah_element_name(b->list, b->ids[i], &name, &err) != ScahStatus_Ok) die("name");
    sink += name.len;
  }
  (void)sink;
}

static void bench_attrs(void *ctx) {
  BenchCtx *b = ctx;
  ScahError *err = NULL;
  ScahStringView key = {.data = (const uint8_t *)"href", .len = 4};
  volatile size_t sink = 0;
  size_t n = b->len < 1000 ? b->len : 1000;
  for (size_t i = 0; i < n; i++) {
    ScahOptionalStringView value = {0};
    if (scah_element_get_attribute(b->list, b->ids[i], key, &value, &err) != ScahStatus_Ok)
      die("get_attribute");
    sink += value.is_some;
  }
  (void)sink;
}

static void bench_attrs_fill(void *ctx) {
  BenchCtx *b = ctx;
  ScahError *err = NULL;
  ScahAttributeView buf[16];
  volatile size_t sink = 0;
  size_t n = b->len < 1000 ? b->len : 1000;
  for (size_t i = 0; i < n; i++) {
    size_t written = 0;
    if (scah_element_attributes_fill(b->list, b->ids[i], buf, 16, &written, &err) != ScahStatus_Ok)
      die("attributes_fill");
    sink += written;
  }
  (void)sink;
}

static void bench_view(void *ctx) {
  BenchCtx *b = ctx;
  ScahError *err = NULL;
  volatile size_t sink = 0;
  size_t n = b->len < 1000 ? b->len : 1000;
  for (size_t i = 0; i < n; i++) {
    ScahElementView view = {0};
    if (scah_element_view(b->list, b->ids[i], &view, &err) != ScahStatus_Ok) die("view");
    sink += view.name.len;
  }
  (void)sink;
}

typedef struct {
  ScahElementList *list;
  const ScahElementId *ids;
  size_t len;
} NestedCtx;

static void bench_nested(void *ctx) {
  NestedCtx *c = ctx;
  ScahError *e = NULL;
  ScahStringView q = {.data = (const uint8_t *)"span", .len = 4};
  for (size_t i = 0; i < c->len; i++) {
    ScahElementList *children = NULL;
    uint8_t f = 0;
    if (scah_element_get(c->list, c->ids[i], q, &children, &f, &e) != ScahStatus_Ok || !f)
      die("element_get");
    scah_element_list_free(children);
  }
}

static void print_stat(const SampleStats *s, int first) {
  if (!first) fputs(",\n", stdout);
  printf(
      "    {\"name\": \"%s\", \"median_ns\": %.3f, \"min_ns\": %.3f, \"p25_ns\": %.3f, "
      "\"p75_ns\": %.3f, \"mad_ns\": %.3f, \"iterations\": %zu, \"samples\": %zu}",
      s->name, s->median_ns, s->min_ns, s->p25_ns, s->p75_ns, s->mad_ns, s->iterations, s->samples);
}

int main(int argc, char **argv) {
  size_t samples = 15;
  size_t fixed_iters = 0;
  int smoke = 0;
  for (int i = 1; i < argc; i++) {
    if (strcmp(argv[i], "--smoke") == 0) {
      smoke = 1;
      samples = 2;
      fixed_iters = 1;
    } else if (strcmp(argv[i], "--samples") == 0 && i + 1 < argc) {
      samples = (size_t)atoi(argv[++i]);
    } else if (strcmp(argv[i], "--iterations") == 0 && i + 1 < argc) {
      fixed_iters = (size_t)atoi(argv[++i]);
    }
  }

  printf("{\n  \"language\": \"c-abi\",\n  \"binding\": \"scah-ffi\",\n  \"samples\": %zu,\n  "
         "\"results\": [\n",
         samples);

  int first = 1;
  size_t sizes[] = {100, 1000, 10000};
  size_t size_count = smoke ? 2 : 3;

  for (size_t si = 0; si < size_count; si++) {
    size_t n = sizes[si];
    char *html = make_html(n);
    ScahQuery *query = build_query("a");
    ScahStore *store = NULL;
    ScahError *err = NULL;
    const ScahQuery *queries[1] = {query};
    ScahStringView html_view = {.data = (const uint8_t *)html, .len = strlen(html)};
    if (scah_parse(html_view, queries, 1, &store, &err) != ScahStatus_Ok) die("parse");
    scah_query_free(query);

    BenchCtx ctx = {.store = store};
    char name[64];
    snprintf(name, sizeof(name), "c_store_get_%zu", n);
    SampleStats s = measure(name, bench_store_get, &ctx, samples, fixed_iters, smoke);
    print_stat(&s, first);
    first = 0;

    ScahElementList *list = NULL;
    uint8_t found = 0;
    ScahStringView q = {.data = (const uint8_t *)"a", .len = 1};
    if (scah_store_get(store, q, &list, &found, &err) != ScahStatus_Ok || !found) die("prep get");
    const ScahElementId *ids = NULL;
    size_t len = 0;
    if (scah_element_list_ids(list, &ids, &len, &err) != ScahStatus_Ok) die("prep ids");
    ctx.list = list;
    ctx.ids = ids;
    ctx.len = len;

    snprintf(name, sizeof(name), "c_list_ids_%zu", n);
    s = measure(name, bench_list_ids, &ctx, samples, fixed_iters, smoke);
    print_stat(&s, 0);

    snprintf(name, sizeof(name), "c_element_name_%zu", n);
    s = measure(name, bench_names, &ctx, samples, fixed_iters, smoke);
    print_stat(&s, 0);

    if (n >= 1000) {
      snprintf(name, sizeof(name), "c_get_attribute_1k_from_%zu", n);
      s = measure(name, bench_attrs, &ctx, samples, fixed_iters, smoke);
      print_stat(&s, 0);

      snprintf(name, sizeof(name), "c_attributes_fill_1k_from_%zu", n);
      s = measure(name, bench_attrs_fill, &ctx, samples, fixed_iters, smoke);
      print_stat(&s, 0);

      snprintf(name, sizeof(name), "c_element_view_1k_from_%zu", n);
      s = measure(name, bench_view, &ctx, samples, fixed_iters, smoke);
      print_stat(&s, 0);
    }

    scah_element_list_free(list);
    scah_store_free(store);
    free(html);
  }

  /* Nested get */
  {
    ScahQueryBuilder *root = NULL;
    ScahQueryBuilder *child = NULL;
    ScahError *err = NULL;
    ScahStringView div = {.data = (const uint8_t *)"div", .len = 3};
    ScahStringView span = {.data = (const uint8_t *)"span", .len = 4};
    if (scah_query_all(div, scah_save_all(), &root, &err) != ScahStatus_Ok) die("nested root");
    if (scah_query_all(span, scah_save_all(), &child, &err) != ScahStatus_Ok) die("nested child");
    ScahQuerySectionId parent = 0;
    if (scah_query_builder_current_section(root, &parent, &err) != ScahStatus_Ok) die("section");
    if (scah_query_builder_append(root, parent, child, &err) != ScahStatus_Ok) die("append");
    ScahQuery *query = NULL;
    if (scah_query_builder_build(root, &query, &err) != ScahStatus_Ok) die("nested build");
    scah_query_builder_free(root);
    scah_query_builder_free(child);

    size_t n = smoke ? 50 : 1000;
    size_t cap = n * 32 + 1;
    char *html = malloc(cap);
    if (!html) abort();
    size_t off = 0;
    for (size_t i = 0; i < n; i++) {
      int wrote = snprintf(html + off, cap - off, "<div><span>c%zu</span></div>", i);
      off += (size_t)wrote;
    }
    ScahStore *store = NULL;
    const ScahQuery *queries[1] = {query};
    ScahStringView html_view = {.data = (const uint8_t *)html, .len = off};
    if (scah_parse(html_view, queries, 1, &store, &err) != ScahStatus_Ok) die("nested parse");
    scah_query_free(query);

    ScahElementList *list = NULL;
    uint8_t found = 0;
    if (scah_store_get(store, div, &list, &found, &err) != ScahStatus_Ok || !found)
      die("nested get");
    const ScahElementId *ids = NULL;
    size_t len = 0;
    if (scah_element_list_ids(list, &ids, &len, &err) != ScahStatus_Ok) die("nested ids");

    NestedCtx nctx = {.list = list, .ids = ids, .len = len};
    SampleStats s = measure("c_nested_element_get", bench_nested, &nctx, samples, fixed_iters, smoke);
    print_stat(&s, 0);

    scah_element_list_free(list);
    scah_store_free(store);
    free(html);
  }

  printf("\n  ]\n}\n");
  return 0;
}
