import { spawnSync } from 'node:child_process'
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'

import * as cheerio from 'cheerio'
import { Window as HappyWindow } from 'happy-dom'
import { JSDOM } from 'jsdom'
import { parseHTML as linkedomParse } from 'linkedom'
import { bench, group, run } from 'mitata'
import { parse as nhpParse } from 'node-html-parser'

import { Query, parse } from '../index.js'

type CliOptions = {
  imageOutput?: string
  jsonOutput?: string
  scenario?: ScenarioName
}

type ScenarioName = 'simple-all' | 'simple-first' | 'whatwg-all-links' | 'nested-all'

type BenchmarkCase = {
  all: (html: string, query: string) => unknown[]
  first: (html: string, query: string) => unknown | null | undefined
}

type ElementSnapshot = {
  innerHtml?: string | null
  rawText?: string | null
  text?: string | null
}

type NestedProductSnapshot = ElementSnapshot & {
  title: ElementSnapshot | null
  rating: ElementSnapshot | null
  description: ElementSnapshot | null
}

type MitataRun = {
  name: string
  stats?: {
    avg: number
    samples?: number[]
  }
}

type MitataBenchmark = {
  alias: string
  runs: MitataRun[]
}

const QUERY = 'a'
const BENCHMARK_NAME = 'node-parse-query'
const DEFAULT_JSON_OUTPUT = './benchmark/results/synthetic.json'
const DEFAULT_IMAGE_OUTPUT = './benchmark/images/synthetic.png'
const SPEC_HTML_FILE = resolve('../../../benches/bench_data/html.spec.whatwg.org.html')
const SIMPLE_HTML = generateHtml(10_000)
const PRODUCT_HTML = generateProductCatalogHtml(10_000)

const CASES: Record<string, BenchmarkCase> = {
  scah: {
    all: (html, query) => {
      const compiledQuery = Query.all(query, { innerHtml: true, rawText: true, text: true }).build()
      const store = parse(html, [compiledQuery])
      return store.get(query) ?? []
    },
    first: (html, query) => {
      const compiledQuery = Query.first(query, { innerHtml: true, rawText: true, text: true }).build()
      const store = parse(html, [compiledQuery])
      return store.get(query)?.[0] ?? null
    },
  },
  cheerio: {
    all: (html, query) => {
      const $ = cheerio.load(html)
      return $(query)
        .toArray()
        .map((element) => ({
          innerHtml: $(element).html(),
          text: $(element).text(),
        }))
    },
    first: (html, query) => {
      const $ = cheerio.load(html)
      const element = $(query).first()
      if (element.length === 0) {
        return null
      }
      return {
        innerHtml: element.html(),
        text: element.text(),
      }
    },
  },
  jsdom: {
    all: (html, query) => {
      const dom = new JSDOM(html)
      return Array.from(dom.window.document.querySelectorAll(query))
    },
    first: (html, query) => {
      const dom = new JSDOM(html)
      return dom.window.document.querySelector(query)
    },
  },
  'node-html-parser': {
    all: (html, query) => {
      const root = nhpParse(html)
      return root.querySelectorAll(query)
    },
    first: (html, query) => {
      const root = nhpParse(html)
      return root.querySelector(query)
    },
  },
  linkedom: {
    all: (html, query) => {
      const { document } = linkedomParse(html)
      return Array.from(document.querySelectorAll(query))
    },
    first: (html, query) => {
      const { document } = linkedomParse(html)
      return document.querySelector(query)
    },
  },
  'happy-dom': {
    all: (html, query) => {
      const window = new HappyWindow()
      window.document.write(html)
      const elements = Array.from(window.document.getElementsByTagName(query))
      const snapshot = elements.map((element) => ({
        innerHtml: element.innerHTML,
        text: element.textContent,
      }))
      window.close()
      return snapshot
    },
    first: (html, query) => {
      const window = new HappyWindow()
      window.document.write(html)
      const element = window.document.getElementsByTagName(query).item(0)
      const snapshot = element
        ? {
            innerHtml: element.innerHTML,
            text: element.textContent,
          }
        : null
      window.close()
      return snapshot
    },
  },
}

function generateHtml(count: number): string {
  let html = "<html><body><div id='content'>"

  for (let i = 0; i < count; i++) {
    html += `<div class="article"><a href="/post/${i}"><b>Post</b> &lt;${i}&gt;</a></div>`
  }

  html += '</div></body></html>'
  return html
}

function generateProductCatalogHtml(count: number): string {
  let html = '<html><body><section id="products">'

  for (let i = 1; i <= count; i++) {
    const rating = ((i - 1) % 5) + 1
    html += `<div class="product"><h1>Product #${i}</h1><span class="rating">${rating}/5</span><p class="description">Description</p></div>`
  }

  html += '</section></body></html>'
  return html
}

function parseCliArgs(argv: string[]): CliOptions {
  const options: CliOptions = {}

  for (let index = 0; index < argv.length; index++) {
    const arg = argv[index]

    if (arg === '--json') {
      const next = argv[index + 1]
      if (next && !next.startsWith('--')) {
        options.jsonOutput = next
        index++
      } else {
        options.jsonOutput = DEFAULT_JSON_OUTPUT
      }
    }

    if (arg === '--image') {
      const next = argv[index + 1]
      if (next && !next.startsWith('--')) {
        options.imageOutput = next
        index++
      } else {
        options.imageOutput = DEFAULT_IMAGE_OUTPUT
      }
    }

    if (arg === '--scenario') {
      const next = argv[index + 1] as ScenarioName | undefined
      if (next) {
        options.scenario = next
        index++
      }
    }
  }

  return options
}

function mean(values: number[]): number {
  return values.reduce((total, value) => total + value, 0) / values.length
}

function standardDeviation(values: number[]): number {
  if (values.length < 2) {
    return 0
  }

  const average = mean(values)
  const variance = values.reduce((total, value) => total + (value - average) ** 2, 0) / values.length
  return Math.sqrt(variance)
}

function toPytestBenchmarkJson(benchmarks: MitataBenchmark[]) {
  return {
    machine_info: {
      runtime: 'bun',
    },
    bench_name: BENCHMARK_NAME,
    benchmarks: benchmarks.flatMap((benchmark) =>
      benchmark.runs.map((run) => {
        const samples = run.stats?.samples ?? []
        const meanNanoseconds = run.stats?.avg ?? 0
        const stddevNanoseconds = standardDeviation(samples)

        return {
          group: benchmark.alias,
          name: run.name,
          fullname: `${benchmark.alias}::${run.name}`,
          stats: {
            mean: meanNanoseconds / 1_000_000_000,
            stddev: stddevNanoseconds / 1_000_000_000,
            rounds: samples.length,
          },
        }
      }),
    ),
  }
}

function writeJsonOutput(outputPath: string, benchmarks: MitataBenchmark[]) {
  const absolutePath = resolve(outputPath)
  mkdirSync(dirname(absolutePath), { recursive: true })
  writeFileSync(absolutePath, JSON.stringify(toPytestBenchmarkJson(benchmarks), null, 2))
  console.log(`Benchmark JSON saved to ${absolutePath}`)
  return absolutePath
}

function renderImageFromJson(jsonPath: string, imagePath: string) {
  const absoluteImagePath = resolve(imagePath)
  mkdirSync(dirname(absoluteImagePath), { recursive: true })

  const figureScript = resolve('../scah-python/benches/utils/figure.py')
  const env = {
    ...process.env,
    MPLCONFIGDIR: process.env.MPLCONFIGDIR ?? '/tmp/matplotlib',
    UV_CACHE_DIR: process.env.UV_CACHE_DIR ?? '/tmp/uv-cache',
  }
  const pythonBindingRoot = resolve('../scah-python')

  const commands: Array<[string, string[]]> = [
    [
      'uv',
      [
        'run',
        '--directory',
        pythonBindingRoot,
        '--all-extras',
        'python3',
        './benches/utils/figure.py',
        jsonPath,
        '-o',
        absoluteImagePath,
      ],
    ],
    ['python3', [figureScript, jsonPath, '-o', absoluteImagePath]],
  ]

  for (const [command, args] of commands) {
    const result = spawnSync(command, args, { env, stdio: 'inherit' })
    if (result.status === 0) {
      console.log(`Benchmark image saved to ${absoluteImagePath}`)
      return
    }

    if (result.error && 'code' in result.error && result.error.code === 'ENOENT') {
      continue
    }
  }

  throw new Error(`Failed to render benchmark image via ${figureScript}`)
}

function consumeElements(elements: unknown[]) {
  for (const element of elements) {
    void readElement(element)
  }
}

function readElement(element: unknown) {
  if (element && typeof element === 'object') {
    if ('innerHtml' in element) {
      void element.innerHtml
    }
    if ('rawText' in element) {
      void element.rawText
    }
    if ('text' in element) {
      void element.text
    }
    if ('textContent' in element) {
      void element.textContent
    }
    if ('innerHTML' in element) {
      void element.innerHTML
    }
  }
}

function snapshotElement(element: unknown): ElementSnapshot | null {
  if (!element || typeof element !== 'object') {
    return null
  }

  if ('innerHtml' in element || 'rawText' in element || 'text' in element || 'textContent' in element) {
    return {
      innerHtml:
        'innerHtml' in element && typeof element.innerHtml !== 'undefined'
          ? (element.innerHtml as string | null)
          : null,
      rawText:
        'rawText' in element && typeof element.rawText !== 'undefined'
          ? (element.rawText as string | null)
          : null,
      text:
        'text' in element && typeof element.text !== 'undefined'
          ? (element.text as string | null)
          : 'textContent' in element && typeof element.textContent !== 'undefined'
            ? (element.textContent as string | null)
            : null,
    }
  }

  if ('innerHTML' in element || 'textContent' in element) {
    return {
      innerHtml:
        'innerHTML' in element && typeof element.innerHTML !== 'undefined'
          ? (element.innerHTML as string | null)
          : null,
      text:
        'textContent' in element && typeof element.textContent !== 'undefined'
          ? (element.textContent as string | null)
          : null,
    }
  }

  return null
}

function consumeNestedProducts(products: NestedProductSnapshot[]) {
  for (const product of products) {
    readElement(product)
    readElement(product.title)
    readElement(product.rating)
    readElement(product.description)
  }
}

function snapshotNestedProducts<T>(
  products: Iterable<T>,
  selectChild: (product: T, selector: string) => unknown | null | undefined,
) {
  const snapshots: NestedProductSnapshot[] = []

  for (const product of products) {
    const productSnapshot = snapshotElement(product)
    snapshots.push({
      innerHtml: productSnapshot?.innerHtml ?? null,
      rawText: productSnapshot?.rawText ?? null,
      text: productSnapshot?.text ?? null,
      title: snapshotElement(selectChild(product, 'h1')),
      rating: snapshotElement(selectChild(product, '.rating')),
      description: snapshotElement(selectChild(product, '.description')),
    })
  }

  return snapshots
}

function registerSimpleAllBenchmarks() {
  group('Simple Selection', () => {
    for (const [name, benchmarkCase] of Object.entries(CASES)) {
      bench(name, () => {
        consumeElements(benchmarkCase.all(SIMPLE_HTML, QUERY))
      })
    }
  })
}

function registerSimpleFirstBenchmarks() {
  group('Simple First Selection', () => {
    for (const [name, benchmarkCase] of Object.entries(CASES)) {
      bench(name, () => {
        const element = benchmarkCase.first(SIMPLE_HTML, QUERY)
        if (element) {
          readElement(element)
        }
      })
    }
  })
}

function registerWhatwgBenchmarks() {
  if (!existsSync(SPEC_HTML_FILE)) {
    console.warn(`Skipping WHATWG benchmark: missing ${SPEC_HTML_FILE}`)
    return
  }

  const specHtml = readFileSync(SPEC_HTML_FILE, 'utf8')
  group('WHATWG All Links', () => {
    for (const [name, benchmarkCase] of Object.entries(CASES)) {
      if (name === 'jsdom' || name === 'happy-dom') {
        continue
      }
      bench(name, () => {
        consumeElements(benchmarkCase.all(specHtml, QUERY))
      })
    }
  })
}

function registerNestedBenchmarks() {
  group('Synthetic Nested Query', () => {
    bench('scah', () => {
      const compiledQuery = Query.all('.product', { innerHtml: true, rawText: true, text: true })
        .then((product) => [
          product.first('> h1', { innerHtml: true, rawText: true, text: true }),
          product.first('> .rating', { innerHtml: true, rawText: true, text: true }),
          product.first('> .description', { innerHtml: true, rawText: true, text: true }),
        ])
        .build()
      const store = parse(PRODUCT_HTML, [compiledQuery])
      const products = store.get('.product') ?? []
      const snapshots = products.map((product) => ({
        innerHtml: product.innerHtml,
        rawText: product.rawText,
        text: product.text,
        title: snapshotElement(product.get('> h1')[0]),
        rating: snapshotElement(product.get('> .rating')[0]),
        description: snapshotElement(product.get('> .description')[0]),
      }))
      consumeNestedProducts(snapshots)
    })

    bench('cheerio', () => {
      const $ = cheerio.load(PRODUCT_HTML)
      const snapshots = $('.product')
        .toArray()
        .map((product) => {
          const node = $(product)
          return {
            innerHtml: node.html(),
            text: node.text(),
            title: {
              innerHtml: node.children('h1').first().html(),
              text: node.children('h1').first().text(),
            },
            rating: {
              innerHtml: node.children('.rating').first().html(),
              text: node.children('.rating').first().text(),
            },
            description: {
              innerHtml: node.children('.description').first().html(),
              text: node.children('.description').first().text(),
            },
          }
        })
      consumeNestedProducts(snapshots)
    })

    bench('jsdom', () => {
      const dom = new JSDOM(PRODUCT_HTML)
      const products = Array.from(dom.window.document.querySelectorAll('.product'))
      const snapshots = snapshotNestedProducts(products, (product, selector) => product.querySelector(selector))
      consumeNestedProducts(snapshots)
    })

    bench('node-html-parser', () => {
      const root = nhpParse(PRODUCT_HTML)
      const snapshots = snapshotNestedProducts(root.querySelectorAll('.product'), (product, selector) =>
        product.querySelector(selector),
      )
      consumeNestedProducts(snapshots)
    })

    bench('linkedom', () => {
      const { document } = linkedomParse(PRODUCT_HTML)
      const products = Array.from(document.querySelectorAll('.product'))
      const snapshots = snapshotNestedProducts(products, (product, selector) => product.querySelector(selector))
      consumeNestedProducts(snapshots)
    })

    // Happy DOM doesn't work in this bench
    // bench('happy-dom', () => {
    //   const window = new HappyWindow()
    //   window.document.write(PRODUCT_HTML)
    //   const products = Array.from(window.document.querySelectorAll('.product'))
    //   const snapshots = snapshotNestedProducts(products, (product, selector) => product.querySelector(selector))
    //   consumeNestedProducts(snapshots)
    //   window.close()
    // })
  })
}

function registerBenchmarks(scenario?: ScenarioName) {
  switch (scenario) {
    case 'simple-all':
      registerSimpleAllBenchmarks()
      return
    case 'simple-first':
      registerSimpleFirstBenchmarks()
      return
    case 'whatwg-all-links':
      registerWhatwgBenchmarks()
      return
    case 'nested-all':
      registerNestedBenchmarks()
      return
    default:
      registerSimpleAllBenchmarks()
      registerSimpleFirstBenchmarks()
      registerWhatwgBenchmarks()
      registerNestedBenchmarks()
  }
}

const options = parseCliArgs(process.argv.slice(2))
registerBenchmarks(options.scenario)

if (!options.jsonOutput && !options.imageOutput) {
  await run()
} else {
  const result = await run({
    format: 'quiet',
  })

  const benchmarks = result.benchmarks as MitataBenchmark[]
  const jsonPath = writeJsonOutput(options.jsonOutput ?? DEFAULT_JSON_OUTPUT, benchmarks)

  if (options.imageOutput) {
    renderImageFromJson(jsonPath, options.imageOutput)
  }
}
