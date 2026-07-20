import { parse, Query } from '../crates/bindings/scah-node/index.js'

function makeHtml(n: number): string {
  let html = ''
  for (let i = 0; i < n; i++) {
    html += `<a href="/${i}" class="c" id="i${i}">t${i}</a>`
  }
  return html
}

function median(samples: number[]): number {
  const sorted = [...samples].sort((a, b) => a - b)
  const mid = Math.floor(sorted.length / 2)
  return sorted.length % 2 === 0 ? (sorted[mid - 1]! + sorted[mid]!) / 2 : sorted[mid]!
}

function timedMedian(fn: () => void, samples = 5, warmup = 2): number {
  for (let i = 0; i < warmup; i++) fn()
  const times: number[] = []
  for (let i = 0; i < samples; i++) {
    const start = performance.now()
    fn()
    times.push((performance.now() - start) / 1000)
  }
  return median(times)
}

const cases: Array<[string, () => void]> = []

for (const n of [100, 1000, 10000]) {
  const html = makeHtml(n)
  const q = Query.all('a', { innerHtml: true, textContent: true }).build()
  const store = parse(html, [q])

  cases.push([
    `lookup_${n}`,
    () => {
      const hits = store.get('a')
      if (!hits) throw new Error('missing')
    },
  ])

  if (n === 10000) {
    cases.push([
      `iterate_name_${n}`,
      () => {
        const hits = store.get('a')!
        for (const el of hits) {
          void el.name
        }
      },
    ])
  }

  if (n >= 1000) {
    cases.push([
      `attrs_1k_from_${n}`,
      () => {
        const hits = store.get('a')!
        for (const el of hits.slice(0, 1000)) {
          void el.attributes
        }
      },
    ])
    cases.push([
      `fields_1k_from_${n}`,
      () => {
        const hits = store.get('a')!
        for (const el of hits.slice(0, 1000)) {
          void el.name
          void el.id
          void el.className
          void el.innerHtml
          void el.textContent
        }
      },
    ])
    cases.push([
      `tojson_1k_from_${n}`,
      () => {
        const hits = store.get('a')!
        for (const el of hits.slice(0, 1000)) {
          void el.toJson()
        }
      },
    ])
  }
}

cases.push([
  'query_nested',
  () => {
    Query.all('div', { innerHtml: true })
      .all('section')
      .all('a', { textContent: true })
      .build()
  },
])

cases.push([
  'query_then',
  () => {
    Query.all('div', { innerHtml: true })
      .then((root) => [
        root.all('a', { textContent: true }),
        root.all('span', { textContent: true }),
        root.all('p', { textContent: true }),
      ])
      .build()
  },
])

console.log('case,median_seconds')
for (const [name, fn] of cases) {
  const m = timedMedian(fn)
  console.log(`${name},${m.toFixed(9)}`)
}
