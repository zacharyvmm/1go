import { test, expect } from 'bun:test'

import { parse, Query } from '../index'

test('Basic selection', () => {
  const html = `
  <div>
    Hello World
    <a href="https://example.com">Example Website</a>
  </div>
  `
  const query = Query.all('div', { innerHtml: true, textContent: true })
    .all('a', { innerHtml: true, textContent: true })
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
    textContent: 'Hello World Example Website',
  })

  let a = div?.get('a').at(0)

  expect(a?.toJson()).toEqual({
    name: 'a',
    class: undefined,
    id: undefined,
    attributes: { href: 'https://example.com' },
    innerHtml: `Example Website`,
    textContent: 'Example Website',
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
  const query = Query.all('#products', { innerHtml: true, textContent: true })
    .all('.product', { innerHtml: true, textContent: true })
    .then((p) => [
      p.all('h1', { innerHtml: true, textContent: true }),
      p.all('img', { innerHtml: false, textContent: false }),
      p.all('p', { innerHtml: true, textContent: true }),
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
  expect(product1.h1.textContent).toBe('Product #1')

  expect(product1.img.name).toBe('img')
  expect(product1.img.attributes).toEqual({ src: 'https://example.com/p1.png' })

  expect(product1.p.name).toBe('p')
  expect(product1.p.textContent).toBe('Hello World for Product #1')

  expect(products[1].name).toBe('div')
  expect(products[1].className).toBe('product')

  const product2 = {
    h1: products[1].get('h1')[0],
    img: products[1].get('img')[0],
    p: products[1].get('p')[0],
  }

  expect(product2.h1.name).toBe('h1')
  expect(product2.h1.innerHtml).toBe('Product #2')
  expect(product2.h1.textContent).toBe('Product #2')

  expect(product2.img.name).toBe('img')
  expect(product2.img.attributes).toEqual({ src: 'https://example.com/p2.png' })

  expect(product2.p.name).toBe('p')
  expect(product2.p.textContent).toBe('Hello World for Product #2')
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
    textContent: true,
  }).build()
  const store = parse(html, [query])

  const links = store.get('a')?.map((e) => e.toJson())

  const generated_links = Array.from({ length: 5000 }, (_, i) => ({
    name: 'a',
    id: undefined,
    class: undefined,
    attributes: { href: `/post/${i}` },
    innerHtml: `<b>Post</b> &lt;${i}&gt;`,
    textContent: `Post &lt;${i}&gt;`,
  }))

  expect(links).toEqual(generated_links)
})

test('Save defaults missing keys to false', () => {
  const html = `<div><span>Hello</span></div>`
  const query = Query.all('div', { textContent: true }).all('span', { innerHtml: true }).build()
  const store = parse(html, [query])

  const div = store.get('div')?.at(0)
  expect(div?.innerHtml).toBeNull()
  expect(div?.textContent).toBe('Hello')

  const span = div?.get('span').at(0)
  expect(span?.innerHtml).toBe('Hello')
  expect(span?.textContent).toBeNull()
})

test('Save defaults omitted object to false', () => {
  const html = `<div><span>Hello</span></div>`
  const query = Query.all('div').all('span').build()
  const store = parse(html, [query])

  const div = store.get('div')?.at(0)
  expect(div?.innerHtml).toBeNull()
  expect(div?.textContent).toBeNull()

  const span = div?.get('span').at(0)
  expect(span?.innerHtml).toBeNull()
  expect(span?.textContent).toBeNull()
})

test('store remains valid after query object goes out of scope', () => {
  // Compiled query data is retained by the FFI store handle, so dropping the
  // JS Query after parse must not invalidate lookups.
  const store = (() => {
    const q = Query.all('a[href]', { innerHtml: true, textContent: true }).build()
    return parse("<a href='x'>x</a>", [q])
  })()

  const hits = store.get('a[href]')
  expect(hits).toHaveLength(1)
  expect(hits![0].name).toBe('a')
  expect(hits![0].attributes).toEqual({ href: 'x' })
})

test('multi-child then appends all branches', () => {
  const html = `<main><a href="1">one</a><span>s</span><p>p</p></main>`
  const query = Query.all('main', { textContent: true })
    .then((q) => [
      q.all('a', { textContent: true }),
      q.all('span', { textContent: true }),
      q.all('p', { textContent: true }),
    ])
    .build()
  const store = parse(html, [query])
  const main = store.get('main')!.at(0)!
  expect(main.get('a')[0].textContent).toBe('one')
  expect(main.get('span')[0].textContent).toBe('s')
  expect(main.get('p')[0].textContent).toBe('p')
})

test('build can be reused without consuming the builder', () => {
  const builder = Query.all('a', { textContent: true })
  const q1 = builder.build()
  const q2 = builder.build()
  const store1 = parse('<a>1</a>', [q1])
  const store2 = parse('<a>2</a>', [q2])
  expect(store1.get('a')![0].textContent).toBe('1')
  expect(store2.get('a')![0].textContent).toBe('2')
})

test('element remains valid after store is dropped', () => {
  const element = (() => {
    const q = Query.all('a', { textContent: true }).build()
    const store = parse('<a>hi</a>', [q])
    return store.get('a')![0]
  })()
  expect(element.name).toBe('a')
  expect(element.textContent).toBe('hi')
})

test('parse with empty queries throws', () => {
  expect(() => parse('<a></a>', [])).toThrow(/parse requires at least one query/)
})

test('invalid selector fails at build', () => {
  expect(() => Query.all('').build()).toThrow()
})

test('attributes preserve missing versus empty values', () => {
  const q = Query.all('input', { innerHtml: true, textContent: true }).build()
  const store = parse('<input disabled value="">', [q])
  const element = store.get('input')![0]
  // napi maps Option::None to null and Some("") to "".
  expect(element.attributes.disabled).toBeNull()
  expect(element.attributes.value).toBe('')
})
