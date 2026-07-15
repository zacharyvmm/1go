import pytest

from scah import Query, Save, parse

HTML = """
<span class="hello" id="world" hello="world">
    Hello <a href="https://www.example.com">World</a>
</span>
<p class="example_class" id="example_id" hello="example">
    My <a href="https://www.example.com">Example</a> or <a href="https://www.notexample.com">Not Example</a>
"""


# ── Binding smoke tests ───────────────────────────────────────────────────


def test_python_binding_basic_parse_and_store_access():
    q = Query.all("a", Save.all()).build()
    store = parse('<a href="x">x</a>', [q])

    hits = store.get("a")
    assert hits is not None
    assert len(hits) == 1
    assert hits[0].name == "a"
    assert hits[0].get_attribute("href") == "x"


def test_nested_selection():
    q = Query.all("#world", Save.all()).all("a", Save.all()).build()
    store = parse(HTML, [q])

    worlds = store.get("#world")
    assert worlds
    assert len(worlds) == 1
    world = dict(worlds[0])

    assert world['id'] == 'world'
    assert world['class'] == 'hello'

    anchors = worlds[0].get('a')
    assert len(anchors) == 1
    anchor = dict(anchors[0])

    assert anchor['name'] == 'a'
    assert 'attributes' in anchor
    assert anchor['attributes']['href'] == "https://www.example.com"
    assert anchor['text_content'] == "World"


def test_branching_selection():
    q = Query.all("#world", Save.all())\
        .then(lambda world: [
            world.all('a', Save.all()), world.all('p', Save.all())
        ]).build()
    store = parse(HTML, [q])

    worlds = store.get("#world")
    assert worlds
    world = worlds[0]

    anchors = world.get("a")
    assert len(anchors) == 1
    assert anchors[0].text_content == "World"


def test_intro():
    html_intro = """
    <div id="project-intro">
        <header>
            <h1 class="title">scah: Streamlined CSS-Selector HTML Extraction</h1>
            <p class="subtitle">A high-performance parsing library built as a bachelor's thesis project.</p>
        </header>
        <article class="overview">
            <p><strong>scah</strong> (<em>scan HTML</em>) bridges the gap between SAX/StAX streaming efficiency and DOM convenience.</p>
            <p>Instead of manually tracking parser state or loading massive documents into memory, you declare your extraction targets using standard CSS selectors.</p>
        </article>

        <aside class="ecosystem">
            <h3>Language Bindings</h3>
            <ul>
                <li class="existing">Python</li>
                <li class="existing">Node.js</li>
                <li class="planned">Unified C API</li>
            </ul>
        </aside>
    </div>
    """

    query_intro = Query.all("div#project-intro", Save.all()) \
        .then(lambda intro: [
            intro.all("article.overview p", Save.all()),
            intro.all("aside.ecosystem li.existing", Save.all())
        ]) \
        .build()

    store_intro = parse(html_intro, [query_intro])

    intro = store_intro.get("div#project-intro")[0]
    assert intro

    p_tags = intro.get("article.overview p")
    assert len(p_tags) == 2
    assert p_tags[0].text_content == "scah ( scan HTML ) bridges the gap between SAX/StAX streaming efficiency and DOM convenience."
    assert p_tags[1].text_content == "Instead of manually tracking parser state or loading massive documents into memory, you declare your extraction targets using standard CSS selectors."

    li_tags = intro.get("aside.ecosystem li.existing")
    assert len(li_tags) == 2
    assert li_tags[0].text_content == "Python"
    assert li_tags[1].text_content == "Node.js"


def test_multiple_root_queries():
    html_api = """
    <main id="api-reference">
        <h2>Supported Selectors</h2>
        <div class="sandbox">
            <span class="badge status-working">Tag Name & Class</span>

            <div id="target-node">ID Selection</div>

            <ul class="combinators">
                <li>Direct Child</li>
                <div>
                    <li>Deep Descendant</li>
                </div>
            </ul>

            <div class="attributes">
                <a href="https://github.com/example" data-type="external">Exact Match & Presence</a>
                <a href="/local/path" data-type="internal">Prefix/Suffix Match</a>
            </div>
        </div>
    </main>
    """

    queries = [
        Query.all("span.status-working", Save.all()).build(),
        Query.all("#target-node", Save.all()).build(),
        Query.all("ul.combinators > li", Save.all()).build(),
        Query.all("ul.combinators li", Save.all()).build(),
        Query.all("a[href][data-type=\"external\"]", Save.all()).build(),
        Query.all("a[href^=\"/\"]", Save.all()).build(),
        Query.first("a", Save.all()).build(),
    ]

    store = parse(html_api, queries)

    span_hits = store.get("span.status-working")
    assert span_hits is not None
    assert len(span_hits) == 1
    assert span_hits[0].text_content == "Tag Name & Class"

    id_hits = store.get("#target-node")
    assert id_hits is not None
    assert len(id_hits) == 1
    assert id_hits[0].id == "target-node"

    child_hits = store.get("ul.combinators > li")
    assert child_hits is not None
    assert len(child_hits) == 1

    desc_hits = store.get("ul.combinators li")
    assert desc_hits is not None
    assert len(desc_hits) == 2

    exact_hits = store.get('a[href][data-type="external"]')
    assert exact_hits is not None
    assert len(exact_hits) == 1
    assert exact_hits[0].get_attribute("href") == "https://github.com/example"

    prefix_hits = store.get('a[href^="/"]')
    assert prefix_hits is not None
    assert len(prefix_hits) == 1
    assert prefix_hits[0].get_attribute("href") == "/local/path"


def test_store_remains_valid_after_query_object_goes_out_of_scope():
    # Query tapes (selector strings) are owned by the query objects.
    # This test verifies that dropping the query does not invalidate
    # the store, because PyStore internally retains _query_tapes.
    def make_store():
        q = Query.all("a[href]", Save.all()).build()
        return parse("<a href='x'>x</a>", [q])

    store = make_store()
    hits = store.get("a[href]")
    assert hits is not None
    assert len(hits) == 1
    assert hits[0].name == "a"
    assert hits[0].get_attribute("href") == "x"


def test_python_query_builder_chaining_smoke():
    q = Query.all("main", Save.none()).all("a", Save.all()).build()
    store = parse("<main><a href='x'>x</a></main>", [q])

    hits = store.get("main")
    assert hits is not None
    assert len(hits) == 1
    anchors = hits[0].get("a")
    assert anchors is not None
    assert len(anchors) == 1
    assert anchors[0].get_attribute("href") == "x"


def test_python_binding_passes_quoted_attribute_selector_smoke():
    selector = '[data-x="a=b"]'
    q = Query.all(selector, Save.all()).build()
    store = parse('<div data-x="a=b"></div>', [q])

    hits = store.get(selector) or []
    assert len(hits) == 1


# ── Python exception mapping tests ─────────────────────────────────────────


def test_build_invalid_selector_raises_value_error_not_panic():
    with pytest.raises(ValueError):
        Query.all("!", Save.none()).build()


def test_try_build_invalid_selector_raises_value_error():
    with pytest.raises(ValueError):
        Query.all("!", Save.none()).try_build()


@pytest.mark.parametrize("selector", [
    '[data-x="unterminated]',
    '[=value]',
    '[data-x^]',
])
def test_invalid_selectors_raise_value_error(selector):
    with pytest.raises(ValueError):
        Query.all(selector, Save.none()).build()
