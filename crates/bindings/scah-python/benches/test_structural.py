import pytest

from bs4 import BeautifulSoup
import lxml.html
from parsel import Selector
from scah import Query, Save, parse
from selectolax.parser import HTMLParser as SelectolaxParser


PRODUCT_COUNT = 10_000




def parse_bs4_lxml(html: str):
    soup = BeautifulSoup(html, "lxml")
    out = []
    for product in soup.select(".product"):
        out.append((
            product.select_one("h1").get_text(strip=True),
            product.select_one(".rating").get_text(strip=True),
            product.select_one(".description").get_text(strip=True),
        ))
    return out


def parse_lxml(html: str):
    tree = lxml.html.fromstring(html)
    out = []
    for product in tree.cssselect(".product"):
        out.append((
            product.cssselect("h1")[0].text_content().strip(),
            product.cssselect(".rating")[0].text_content().strip(),
            product.cssselect(".description")[0].text_content().strip(),
        ))
    return out


def parse_selectolax(html: str):
    tree = SelectolaxParser(html)
    out = []
    for product in tree.css(".product"):
        out.append((
            product.css_first("h1").text(strip=True),
            product.css_first(".rating").text(strip=True),
            product.css_first(".description").text(strip=True),
        ))
    return out


def parse_parsel(html: str):
    selector = Selector(text=html)
    out = []
    for product in selector.css(".product"):
        out.append((
            product.css("h1::text").get(),
            product.css(".rating::text").get(),
            product.css(".description::text").get(),
        ))
    return out


def parse_scah(html: str):
    q = (
        Query.all(".product", Save.all())
        .then(lambda product: [
            product.all("> h1", Save.all()),
            product.all("> .rating", Save.all()),
            product.all("> .description", Save.all()),
        ])
        .build()
    )

    store = parse(html, [q])
    products = store.get(".product")
    out = []

    for product in products:
        out.append((
            product.get("> h1")[0].text,
            product.get("> .rating")[0].text,
            product.get("> .description")[0].text,
        ))

    return out


PARSERS = {
    "Scah": parse_scah,
    "lxml": parse_lxml,
    "BS4 (lxml)": parse_bs4_lxml,
    "Selectolax": parse_selectolax,
    "Parsel": parse_parsel,
}


@pytest.fixture(scope="session")
def html_content():
    products = "".join(
        f"""
        <div class="product">
            <h1>Product {i}</h1>
            <span class="rating">{(i % 5) + 1}/5</span>
            <p class="description">Description {i}</p>
            <div class="meta">
                <span class="sku">SKU-{i}</span>
                <a href="/products/{i}">View</a>
            </div>
        </div>
        """
        for i in range(PRODUCT_COUNT)
    )
    return f"<html><body><main>{products}</main></body></html>"


@pytest.mark.parametrize("name", PARSERS.keys())
def test_structured_benchmark(benchmark, html_content, name):
    lib = PARSERS[name]

    benchmark.group = f"Synthetic Nested Query ({PRODUCT_COUNT} products)"
    benchmark.name = name

    result = benchmark(lib, html_content)
    assert len(result) == PRODUCT_COUNT
