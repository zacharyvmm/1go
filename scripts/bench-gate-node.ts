#!/usr/bin/env bun
/** Focused Node gate benchmarks (low wall-clock). */

import { parse, Query } from '../crates/bindings/scah-node/index.js'
import { writeFileSync } from 'node:fs'
import os from 'node:os'

function percentile(sorted: number[], p: number): number {
  if (!sorted.length) return 0
  const k = (sorted.length - 1) * p
  const f = Math.floor(k)
  const c = Math.min(f + 1, sorted.length - 1)
  if (f === c) return sorted[f]!
  return sorted[f]! + (sorted[c]! - sorted[f]!) * (k - f)
}

function calibrate(fn: () => void, targetS = 0.05): number {
  let iterations = 1
  while (true) {
    const start = performance.now()
    for (let i = 0; i < iterations; i++) fn()
    const elapsed = (performance.now() - start) / 1000
    if (elapsed >= targetS || iterations >= 500_000) return iterations
    iterations *= 2
  }
}

function measure(name: string, fn: () => void, samples: number) {
  const iters = calibrate(fn)
  for (let w = 0; w < 2; w++) for (let i = 0; i < iters; i++) fn()
  const perOp: number[] = []
  for (let s = 0; s < samples; s++) {
    const start = performance.now()
    for (let i = 0; i < iters; i++) fn()
    perOp.push(((performance.now() - start) * 1e6) / iters)
  }
  const ordered = [...perOp].sort((a, b) => a - b)
  const med = percentile(ordered, 0.5)
  return {
    name,
    median_ns: med,
    min_ns: ordered[0]!,
    p25_ns: percentile(ordered, 0.25),
    p75_ns: percentile(ordered, 0.75),
    mad_ns: percentile(
      ordered.map((v) => Math.abs(v - med)).sort((a, b) => a - b),
      0.5,
    ),
    iterations: iters,
    samples,
  }
}

function makeHtml(n: number): string {
  let html = ''
  for (let i = 0; i < n; i++) html += `<a href="/${i}" class="c" id="i${i}">t${i}</a>`
  return html
}

const samples = (() => {
  const i = process.argv.indexOf('--samples')
  return i >= 0 ? Number(process.argv[i + 1]) : 7
})()
const outIdx = process.argv.indexOf('--output')
const output = outIdx >= 0 ? process.argv[outIdx + 1]! : '-'
const skip100k = process.argv.includes('--skip-100k')

const results: ReturnType<typeof measure>[] = []
const sizes = [100, 1000, 10000].concat(skip100k ? [] : [100000])

for (const n of sizes) {
  const store = parse(makeHtml(n), [Query.all('a', { innerHtml: true, textContent: true }).build()])
  results.push(
    measure(
      `lookup_${n}`,
      () => {
        const hits = store.get('a')
        if (!hits) throw new Error('missing')
        let t = 0
        for (const el of hits) t += el.name.length
        if (!t) throw new Error('empty')
      },
      samples,
    ),
  )
  if (n === 1000) {
    const subset = store.get('a')!.slice(0, 1000)
    results.push(
      measure(
        'iterate_1000',
        () => {
          for (const _ of subset) {
            /* */
          }
        },
        samples,
      ),
    )
    results.push(
      measure(
        'field_name_1k_from_1000',
        () => {
          for (const el of subset) void el.name
        },
        samples,
      ),
    )
    results.push(
      measure(
        'attrs_1k_from_1000',
        () => {
          for (const el of subset) void el.attributes
        },
        samples,
      ),
    )
    results.push(
      measure(
        'toJson_1k_from_1000',
        () => {
          for (const el of subset) void el.toJson()
        },
        samples,
      ),
    )
  }
}

const parents = parse(
  Array.from({ length: 500 }, (_, i) => `<div><span>c${i}</span></div>`).join(''),
  [Query.all('div', { textContent: true }).all('span', { textContent: true }).build()],
).get('div')!
results.push(
  measure(
    'nested_lookup',
    () => {
      for (const p of parents) {
        if (!p.get('span').length) throw new Error('missing')
      }
    },
    samples,
  ),
)

for (const [label, size] of [
  ['parse_10kb', 10_000],
  ['parse_100kb', 100_000],
  ['parse_1mb', 1_000_000],
] as const) {
  const html = ('<div>' + '<a>x</a>'.repeat(Math.floor(size / 8)) + '</div>').slice(0, size)
  const q = Query.all('a', {}).build()
  results.push(
    measure(
      label,
      () => {
        parse(html, [q])
      },
      samples,
    ),
  )
}

const payload = {
  language: 'node',
  runtime: typeof Bun !== 'undefined' ? `bun ${Bun.version}` : process.version,
  platform: `${os.type()} ${os.release()}`,
  machine: os.arch(),
  samples,
  results,
}
const text = JSON.stringify(payload, null, 2)
if (output === '-') console.log(text)
else writeFileSync(output, text + '\n')
console.error(`wrote ${output} (${results.length} cases)`)
