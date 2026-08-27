import pytest

from scah import Query, Save, parse


# ── API smoke test ─────────────────────────────────────────────────────────


def test_basic_parse_and_store_access():
    """Verify Python can construct a query, build it, parse HTML, and
    access a stored element's attributes and properties."""
    q = Query.all("a", Save.all()).build()
    store = parse('<a href="x">link</a>', [q])

    hits = store.get("a")
    assert hits is not None
    assert len(hits) == 1
    assert hits[0].name == "a"
    assert hits[0].get_attribute("href") == "x"


# ── Builder chaining exposure ──────────────────────────────────────────────


def test_builder_chaining():
    """Verify that chained builder calls produce usable Python objects and
    expose nested store results correctly."""
    q = Query.all("main", Save.none()).all("a", Save.all()).build()
    store = parse("<main><a href='x'>x</a></main>", [q])

    main_hits = store.get("main")
    assert main_hits is not None
    assert len(main_hits) == 1

    anchors = main_hits[0].get("a")
    assert anchors is not None
    assert len(anchors) == 1
    assert anchors[0].get_attribute("href") == "x"


# ── .then() callback conversion ────────────────────────────────────────────


def test_then_callback():
    """Verify that Python receives the query factory in .then(), the
    callback can return a list of child builders, and the resulting
    nested store is accessible."""
    q = (
        Query.all("#root", Save.all())
        .then(lambda root: [root.all("a", Save.all())])
        .build()
    )
    store = parse('<div id="root"><a href="x">link</a></div>', [q])

    roots = store.get("#root")
    assert roots and len(roots) == 1

    anchors = roots[0].get("a")
    assert anchors and len(anchors) == 1
    assert anchors[0].get_attribute("href") == "x"


# ── Multiple-root-query marshalling ────────────────────────────────────────


def test_multiple_root_queries():
    """Verify that Python can pass multiple built queries to parse() and
    retrieve their corresponding entries."""
    q1 = Query.all("a", Save.all()).build()
    q2 = Query.first("span", Save.all()).build()

    html = '<a href="a">A</a><span class="badge">B</span>'
    store = parse(html, [q1, q2])

    hits1 = store.get("a")
    assert hits1 is not None
    assert len(hits1) == 1
    assert hits1[0].get_attribute("href") == "a"

    hits2 = store.get("span")
    assert hits2 is not None
    assert len(hits2) == 1
    assert hits2[0].text == "B"


# ── Query/store lifetime safety ────────────────────────────────────────────


def test_store_remains_valid_after_query_object_goes_out_of_scope():
    """Verify that the store remains valid after the Python query object
    goes out of scope (PyStore retains the query tapes)."""
    def make_store():
        q = Query.all("a[href]", Save.all()).build()
        return parse("<a href='x'>x</a>", [q])

    store = make_store()
    hits = store.get("a[href]")
    assert hits is not None
    assert len(hits) == 1
    assert hits[0].name == "a"
    assert hits[0].get_attribute("href") == "x"


# ── Exception mapping ──────────────────────────────────────────────────────


def test_build_invalid_selector_raises_value_error():
    with pytest.raises(ValueError):
        Query.all("!", Save.none()).build()


def test_query_builder_does_not_expose_try_build():
    builder = Query.all("a", Save.none())
    assert not hasattr(builder, "try_build")


def test_raw_text_and_text_properties():
    store = parse("<p>A&nbsp;&amp;&#x20;B</p>", [Query.all("p", Save.all()).build()])
    p = store.get("p")[0]
    assert p.raw_text == "A&nbsp;&amp;&#x20;B"
    assert p.text == "A & B"
    assert p["raw_text"] == "A&nbsp;&amp;&#x20;B"
    assert p["text"] == "A & B"
    assert p.keys() == [
        "name",
        "id",
        "class",
        "attributes",
        "inner_html",
        "raw_text",
        "text",
    ]


def test_empty_versus_uncaptured_text():
    empty = parse("<div></div>", [Query.all("div", Save.all()).build()]).get("div")[0]
    assert empty.raw_text == ""
    assert empty.text == ""

    text_only = parse("<input>", [Query.all("input", Save.only_text()).build()]).get(
        "input"
    )[0]
    assert text_only.text == ""
    assert text_only.raw_text is None


def test_save_helpers():
    store = parse(
        "<p>hi</p>",
        [Query.all("p", Save.only_raw_text()).build()],
    )
    p = store.get("p")[0]
    assert p.raw_text == "hi"
    assert p.text is None

    store = parse("<p>hi</p>", [Query.all("p", Save.only_text()).build()])
    p = store.get("p")[0]
    assert p.text == "hi"
    assert p.raw_text is None


def test_save_positional_text_compatibility():
    store = parse("<p>A&nbsp;B</p>", [Query.all("p", Save(False, True)).build()])
    p = store.get("p")[0]
    assert p.text == "A B"
    assert p.raw_text is None


def test_raw_text_is_keyword_only():
    store = parse(
        "<p>A&nbsp;B</p>",
        [Query.all("p", Save(raw_text=True)).build()],
    )
    p = store.get("p")[0]
    assert p.raw_text == "A&nbsp;B"
    assert p.text is None
