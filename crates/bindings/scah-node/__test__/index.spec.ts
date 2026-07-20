import { test, expect } from 'bun:test'

import { parse, Query } from '../index'

test('Basic selection', () => {
  const html = `
  <div>
    Hello World
    <a href="https://example.com">Example Website</a>
  </div>
  `
  const query = Query.all('div', { innerHtml: true, text: true })
    .all('a', { innerHtml: true, text: true })
    .build()
  const store = parse(html, [query])

  expect(store.length).toBe(2)

  expect(store.get('div')?.length).toBe(1)

  let div = store.get('div')?.at(0)
  expect(div?.toJson()).toEqual({
    name: 'div',
    class: undefined,
    id: undefined,
    attributes: {},
    innerHtml: `
    Hello World
    <a href="https://example.com">Example Website</a>
  `,
    rawText: undefined,
    text: 'Hello World Example Website',
  })

  let a = div?.get('a').at(0)

  expect(a?.toJson()).toEqual({
    name: 'a',
    class: undefined,
    id: undefined,
    attributes: { href: 'https://example.com' },
    innerHtml: `Example Website`,
    rawText: undefined,
    text: 'Example Website',
  })
})

test('Tree selection', () => {
  const html = `
  <section id="products">
    <div class="product">
      <h1>Product #1</h1>
      <img src="https://example.com/p1.png"/>
      <p>
        Hello World for Product #1
      </p>
    </div>
    <div class="product">
      <h1>Product #2</h1>
      <img src="https://example.com/p2.png"/>
      <p>
        Hello World for Product #2
      </p>
    </div>
  </section>
  `
  const query = Query.all('#products', { innerHtml: true, text: true })
    .all('.product', { innerHtml: true, text: true })
    .then((p) => [
      p.all('h1', { innerHtml: true, text: true }),
      p.all('img', { innerHtml: false, text: false }),
      p.all('p', { innerHtml: true, text: true }),
    ])
    .build()
  const store = parse(html, [query])

  expect(store.length).toBe(9)

  const products_section = store.get('#products')
  expect(products_section?.length).toBe(1)

  expect(products_section![0]?.name).toBe('section')
  expect(products_section![0]?.id).toBe('products')

  const products = products_section![0].get('.product')!

  expect(products[0].name).toBe('div')
  expect(products[0].className).toBe('product')

  const product1 = {
    h1: products[0].get('h1')[0],
    img: products[0].get('img')[0],
    p: products[0].get('p')[0],
  }
  expect(product1.h1.name).toBe('h1')
  expect(product1.h1.innerHtml).toBe('Product #1')
  expect(product1.h1.text).toBe('Product #1')

  expect(product1.img.name).toBe('img')
  expect(product1.img.attributes).toEqual({ src: 'https://example.com/p1.png' })

  expect(product1.p.name).toBe('p')
  expect(product1.p.text).toBe('Hello World for Product #1')

  expect(products[1].name).toBe('div')
  expect(products[1].className).toBe('product')

  const product2 = {
    h1: products[1].get('h1')[0],
    img: products[1].get('img')[0],
    p: products[1].get('p')[0],
  }

  expect(product2.h1.name).toBe('h1')
  expect(product2.h1.innerHtml).toBe('Product #2')
  expect(product2.h1.text).toBe('Product #2')

  expect(product2.img.name).toBe('img')
  expect(product2.img.attributes).toEqual({ src: 'https://example.com/p2.png' })

  expect(product2.p.name).toBe('p')
  expect(product2.p.text).toBe('Hello World for Product #2')
})

function generateHtml(count: number): string {
  let html = "<html><body><div id='content'>"

  for (let i = 0; i < count; i++) {
    // Added some entities (&lt;) and bold tags (<b>) to make text extraction work harder
    html += `<div class="article"><a href="/post/${i}"><b>Post</b> &lt;${i}&gt;</a></div>`
  }

  html += '</div></body></html>'
  return html
}
test('find 5_000 anchor tags', () => {
  const html = generateHtml(5000)
  const query = Query.all('a', {
    innerHtml: true,
    text: true,
  }).build()
  const store = parse(html, [query])

  const links = store.get('a')?.map((e) => e.toJson())

  const generated_links = Array.from({ length: 5000 }, (_, i) => ({
    name: 'a',
    id: undefined,
    class: undefined,
    attributes: { href: `/post/${i}` },
    innerHtml: `<b>Post</b> &lt;${i}&gt;`,
    rawText: undefined,
    text: `Post <${i}>`,
  }))

  expect(links).toEqual(generated_links)
})

test('Save defaults missing keys to false', () => {
  const html = `<div><span>Hello</span></div>`
  const query = Query.all('div', { text: true }).all('span', { innerHtml: true }).build()
  const store = parse(html, [query])

  const div = store.get('div')?.at(0)
  expect(div?.innerHtml).toBeNull()
  expect(div?.text).toBe('Hello')

  const span = div?.get('span').at(0)
  expect(span?.innerHtml).toBe('Hello')
  expect(span?.text).toBeNull()
})

test('Save defaults omitted object to false', () => {
  const html = `<div><span>Hello</span></div>`
  const query = Query.all('div').all('span').build()
  const store = parse(html, [query])

  const div = store.get('div')?.at(0)
  expect(div?.innerHtml).toBeNull()
  expect(div?.text).toBeNull()

  const span = div?.get('span').at(0)
  expect(span?.innerHtml).toBeNull()
  expect(span?.text).toBeNull()
})

test("store remains valid after query object goes out of scope", () => {
  // Query tapes (selector strings) are owned by the query objects.
  // This test verifies that dropping the query does not invalidate
  // the store, because JSStore internally retains _query_tapes.
  const store = (() => {
    const q = Query.all("a[href]", { innerHtml: true, text: true }).build()
    return parse("<a href='x'>x</a>", [q])
  })()

  const hits = store.get("a[href]")
  expect(hits).toHaveLength(1)
  expect(hits![0].name).toBe("a")
  expect(hits![0].attributes).toEqual({ href: "x" })
})

test('rawText preserves entities while text decodes them', () => {
  const html = '<p>A&nbsp;&amp;&#x20;B</p>'
  const query = Query.all('p', { rawText: true, text: true }).build()
  const store = parse(html, [query])
  const p = store.get('p')?.at(0)
  expect(p?.rawText).toBe('A&nbsp;&amp;&#x20;B')
  expect(p?.text).toBe('A & B')
})

test('requested empty text is empty string not null', () => {
  const html = '<div></div>'
  const query = Query.all('div', { rawText: true, text: true }).build()
  const store = parse(html, [query])
  const div = store.get('div')?.at(0)
  expect(div?.rawText).toBe('')
  expect(div?.text).toBe('')
})

test('Save static helpers map correctly', () => {
  const { Save } = require('../index')
  expect(Save.onlyRawText()).toEqual({ innerHtml: false, rawText: true, text: false })
  expect(Save.onlyText()).toEqual({ innerHtml: false, rawText: false, text: true })
})
