import os
import re
from io import BytesIO

from html.parser import HTMLParser
from bs4 import BeautifulSoup
import lxml.html
from selectolax.parser import HTMLParser as SelectolaxParser
from parsel import Selector
from gazpacho import Soup as GazpachoSoup
import scah

def _consume_bs4(elements):
    out = []
    for element in elements:
        out.append((
            dict(element.attrs),
            element.decode_contents(),
            element.get_text(),
        ))
    return out

def _consume_lxml(elements):
    out = []
    for element in elements:
        out.append((
            dict(element.attrib),
            element.text_content(),
            lxml.html.tostring(element, encoding="unicode"),
        ))
    return out

def _consume_selectolax(elements):
    out = []
    for element in elements:
        out.append((
            dict(element.attributes),
            element.html,
            element.text(),
        ))
    return out

def _consume_parsel(elements):
    out = []
    for element in elements:
        out.append((
            element.attrib,
            element.get(),
            element.xpath("string()").get(),
        ))
    return out

def _consume_gazpacho(elements):
    out = []
    for element in elements:
        out.append((
            dict(element.attrs),
            str(element),
            element.text,
        ))
    return out

def _consume_scah(elements):
    out = []
    for element in elements:
        out.append((
            element.attributes,
            element.inner_html,
            element.text,
        ))
    return out

def parse_bs4_htmlparser(html:str, query:str):
    soup = BeautifulSoup(html, "html.parser")
    return _consume_bs4(soup.find_all(query))

def parse_bs4_htmlparser_first(html:str, query:str):
    soup = BeautifulSoup(html, "html.parser")
    element = soup.find(query)
    return [element] if element is not None else []

def parse_bs4_lxml(html:str, query:str):
    soup = BeautifulSoup(html, "lxml")
    return _consume_bs4(soup.find_all(query))

def parse_bs4_lxml_first(html:str, query:str):
    soup = BeautifulSoup(html, "lxml")
    element = soup.find(query)
    return [element] if element is not None else []

def parse_lxml(html:str, query:str):
    tree = lxml.html.fromstring(html)
    return _consume_lxml(tree.cssselect(query))

def parse_lxml_first(html:str, query:str):
    tree = lxml.html.fromstring(html)
    elements = tree.cssselect(query)
    return elements[:1]

def parse_selectolax(html:str, query:str):
    tree = SelectolaxParser(html)
    return _consume_selectolax(tree.css(query))

def parse_selectolax_first(html:str, query:str):
    tree = SelectolaxParser(html)
    element = tree.css_first(query)
    return [element] if element is not None else []

def parse_parsel(html:str, query:str):
    selector = Selector(text=html)
    return _consume_parsel(selector.css(query))

def parse_parsel_first(html:str, query:str):
    selector = Selector(text=html)
    element = selector.css(query).get()
    return [element] if element is not None else []

def parse_gazpacho(html:str, query:str):
    soup = GazpachoSoup(html)
    return _consume_gazpacho(soup.find(query, mode='all'))

def parse_gazpacho_first(html:str, query:str):
    soup = GazpachoSoup(html)
    element = soup.find(query)
    return [element] if element is not None else []

def parse_scah(html: str, query:str):
    q = scah.Query.all(query, scah.Save.all()).build()
    store = scah.parse(html, [q])
    return _consume_scah(store.get(query))

def parse_scah_first(html: str, query:str):
    q = scah.Query.first(query, scah.Save.all()).build()
    store = scah.parse(html, [q])
    return store.get(query)

ALL_PARSERS = {
    "Scah": parse_scah,
    # "BS4 (html.parser)": parse_bs4_htmlparser,
    "lxml": parse_lxml,
    "BS4 (lxml)": parse_bs4_lxml,
    "Selectolax": parse_selectolax,
    "Parsel": parse_parsel,
    "Gazpacho": parse_gazpacho,
}

FIRST_PARSERS = {
    "Scah": parse_scah_first,
    # "BS4 (html.parser)": parse_bs4_htmlparser_first,
    "lxml": parse_lxml_first,
    "BS4 (lxml)": parse_bs4_lxml_first,
    "Selectolax": parse_selectolax_first,
    "Parsel": parse_parsel_first,
    "Gazpacho": parse_gazpacho_first,
}

def get_available_parsers(mode: str = "all"):
    if mode == "all":
        return ALL_PARSERS
    if mode == "first":
        return FIRST_PARSERS
    raise ValueError(f"Unknown parser mode: {mode}")
