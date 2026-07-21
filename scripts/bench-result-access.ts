#!/usr/bin/env bun
/**
 * Calibrated result-access benchmarks for the Node scah binding.
 * Emits machine-readable JSON. Property phases precompute `store.get()`.
 */

import { parse, Query } from '../crates/bindings/scah-node/index.js'
import { writeFileSync } from 'node:fs'
import os from 'node:os'

type SampleStats = {
  name: string
  median_ns: number
  min_ns: number
  p25_ns: number
  p75_ns: number
  mad_ns: number
  iterations: number
  samples: number
}

function percentile(sorted: number[], p: number): number {
  if (sorted.length === 0) return 0
  const k = (sorted.length - 1) * p
  const f = Math.floor(k)
  const c = Math.min(f + 1, sorted.length - 1)
  if (f === c) return sorted[f]!
  return sorted[f]! + (sorted[c]! - sorted[f]!) * (k - f)
}

function mad(values: number[], med: number): number {
  const deviations = values.map((v) => Math.abs(v - med)).sort((a, b) => a - b)
  return percentile(deviations, 0.5)
}

function calibrate(fn: () => void, targetS = 0.1): number {
  let iterations = 1
  while (true) {
    const start = performance.now()
    for (let i = 0; i < iterations; i++) fn()
    const elapsed = (performance.now() - start) / 1000
    if (elapsed >= targetS || iterations >= 1_000_000) return iterations
    iterations *= 2
  }
}

function measure(
  name: string,
  fn: () => void,
  samples: number,
  iterations: number | null,
  targetS: number,
): SampleStats {
  const iters = iterations ?? calibrate(fn, targetS)
  for (let w = 0; w < Math.min(3, samples); w++) {
    for (let i = 0; i < iters; i++) fn()
  }

  const perOp: number[] = []
  for (let s = 0; s < samples; s++) {
    const start = performance.now()
    for (let i = 0; i < iters; i++) fn()
    const elapsedNs = (performance.now() - start) * 1e6
    perOp.push(elapsedNs / iters)
  }

  const ordered = [...perOp].sort((a, b) => a - b)
  const med = percentile(ordered, 0.5)
  return {
    name,
    median_ns: med,
    min_ns: ordered[0]!,
    p25_ns: percentile(ordered, 0.25),
    p75_ns: percentile(ordered, 0.75),
    mad_ns: mad(ordered, med),
    iterations: iters,
    samples,
  }
}

function makeHtml(n: number): string {
  let html = ''
  for (let i = 0; i < n; i++) {
    html += `<a href="/${i}" class="c" id="i${i}" data-k="${i}">t${i}</a>`
  }
  return html
}

function consumeHits(hits: Array<{ name: string }>): number {
  let total = 0
  for (const el of hits) total += el.name.length
  return total
}

function parseArgs(argv: string[]) {
  let samples = 15
  let iterations: number | null = null
  let targetMs = 100
  let output = '-'
  let smoke = false
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i]!
    if (arg === '--samples') samples = Number(argv[++i])
    else if (arg === '--iterations') {
      const v = argv[++i]!
      iterations = v === 'auto' ? null : Number(v)
    } else if (arg === '--target-ms') targetMs = Number(argv[++i])
    else if (arg === '--output') output = argv[++i]!
    else if (arg === '--smoke') smoke = true
  }
  if (smoke) {
    samples = 2
    if (iterations === null) iterations = 1
    targetMs = 10
  }
  return { samples, iterations, targetMs, output, smoke }
}

const args = parseArgs(process.argv.slice(2))
const targetS = args.targetMs / 1000
const results: SampleStats[] = []

for (const n of [100, 1000, 10000, 100000]) {
  if (args.smoke && n > 1000) continue
  const html = makeHtml(n)
  const q = Query.all('a', { innerHtml: true, textContent: true }).build()
  const store = parse(html, [q])

  results.push(
    measure(
      `lookup_${n}`,
      () => {
        const hits = store.get('a')
        if (!hits) throw new Error('missing')
        consumeHits(hits)
      },
      args.samples,
      args.iterations,
      targetS,
    ),
  )

  const hits = store.get('a')!
  results.push(
    measure(
      `iterate_${n}`,
      () => {
        for (const _el of hits) {
          /* consume */
        }
      },
      args.samples,
      args.iterations,
      targetS,
    ),
  )

  if (n >= 1000) {
    const subset = hits.slice(0, 1000)
    const cases: Array<[string, () => void]> = [
      [
        `field_name_1k_from_${n}`,
        () => {
          for (const el of subset) void el.name
        },
      ],
      [
        `field_id_1k_from_${n}`,
        () => {
          for (const el of subset) void el.id
        },
      ],
      [
        `field_class_1k_from_${n}`,
        () => {
          for (const el of subset) void el.className
        },
      ],
      [
        `field_text_1k_from_${n}`,
        () => {
          for (const el of subset) void el.textContent
        },
      ],
      [
        `field_inner_1k_from_${n}`,
        () => {
          for (const el of subset) void el.innerHtml
        },
      ],
      [
        `field_attr_href_1k_from_${n}`,
        () => {
          for (const el of subset) void el.getAttribute('href')
        },
      ],
      [
        `attrs_1k_from_${n}`,
        () => {
          for (const el of subset) void el.attributes
        },
      ],
      [
        `toJson_1k_from_${n}`,
        () => {
          for (const el of subset) void el.toJson()
        },
      ],
    ]
    for (const [name, fn] of cases) {
      results.push(measure(name, fn, args.samples, args.iterations, targetS))
    }
  }
}

const nestedCount = args.smoke ? 50 : 1000
let nestedHtml = ''
for (let i = 0; i < nestedCount; i++) nestedHtml += `<div><span>c${i}</span></div>`
const nestedQ = Query.all('div', { textContent: true }).all('span', { textContent: true }).build()
const nestedStore = parse(nestedHtml, [nestedQ])
const parents = nestedStore.get('div')!
results.push(
  measure(
    'nested_lookup',
    () => {
      for (const parent of parents) {
        const children = parent.get('span')
        if (!children.length) throw new Error('missing child')
      }
    },
    args.samples,
    args.iterations,
    targetS,
  ),
)

for (const [name, fn] of [
  [
    'query_simple',
    () => {
      Query.all('a', { textContent: true }).build()
    },
  ],
  [
    'query_nested',
    () => {
      Query.all('div', { textContent: true })
        .all('section', {})
        .all('a', { textContent: true })
        .build()
    },
  ],
  [
    'query_then',
    () => {
      Query.all('div', { textContent: true })
        .then((root) => [
          root.all('a', { textContent: true }),
          root.all('span', { textContent: true }),
          root.all('p', { textContent: true }),
        ])
        .build()
    },
  ],
] as Array<[string, () => void]>) {
  results.push(measure(name, fn, args.samples, args.iterations, targetS))
}

for (const [label, size] of [
  ['parse_10kb', 10_000],
  ['parse_100kb', 100_000],
  ['parse_1mb', 1_000_000],
] as Array<[string, number]>) {
  if (args.smoke && size > 10_000) continue
  const html = ('<div>' + '<a>x</a>'.repeat(Math.floor(size / 8)) + '</div>').slice(0, size)
  const q = Query.all('a', {}).build()
  results.push(
    measure(
      label,
      () => {
        parse(html, [q])
      },
      args.samples,
      args.iterations,
      targetS,
    ),
  )
}

const payload = {
  language: 'node',
  binding: '@zacharymm/scah',
  runtime: typeof Bun !== 'undefined' ? `bun ${Bun.version}` : `node ${process.version}`,
  platform: `${os.type()} ${os.release()}`,
  machine: os.arch(),
  samples: args.samples,
  iteration_strategy:
    args.iterations === null ? 'auto-calibrated >= target' : `fixed=${args.iterations}`,
  target_ms: args.targetMs,
  results,
}

const text = JSON.stringify(payload, null, 2)
if (args.output === '-') console.log(text)
else writeFileSync(args.output, text + '\n')
