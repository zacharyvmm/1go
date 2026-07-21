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


def test_then_with_multiple_children():
    """Verify .then() can append multiple sibling child builders."""
    q = (
        Query.all("div", Save.all())
        .then(
            lambda root: [
                root.all("a", Save.all()),
                root.all("span", Save.all()),
            ]
        )
        .build()
    )
    store = parse("<div><a href='1'>A</a><span>S</span></div>", [q])

    roots = store.get("div")
    assert roots and len(roots) == 1
    assert len(roots[0].get("a")) == 1
    assert roots[0].get("a")[0].get_attribute("href") == "1"
    assert len(roots[0].get("span")) == 1
    assert roots[0].get("span")[0].text_content == "S"


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
    assert hits2[0].text_content == "B"


# ── Query/store lifetime safety ────────────────────────────────────────────


def test_store_remains_valid_after_query_object_goes_out_of_scope():
    """Verify that the store remains valid after the Python query object
    goes out of scope (store owns its backing data via FFI)."""
    def make_store():
        q = Query.all("a[href]", Save.all()).build()
        return parse("<a href='x'>x</a>", [q])

    store = make_store()
    hits = store.get("a[href]")
    assert hits is not None
    assert len(hits) == 1
    assert hits[0].name == "a"
    assert hits[0].get_attribute("href") == "x"


def test_query_survives_builder_destruction():
    """Compiled query remains usable after its builder is dropped."""
    def make_query():
        builder = Query.all("a", Save.all())
        return builder.build()

    q = make_query()
    store = parse('<a href="x">x</a>', [q])
    hits = store.get("a")
    assert hits is not None
    assert hits[0].get_attribute("href") == "x"


def test_build_reuse():
    """Building from the same builder multiple times yields independent queries."""
    builder = Query.all("a", Save.all())
    q1 = builder.build()
    q2 = builder.build()

    store1 = parse('<a href="1">one</a>', [q1])
    store2 = parse('<a href="2">two</a>', [q2])

    assert store1.get("a")[0].get_attribute("href") == "1"
    assert store2.get("a")[0].get_attribute("href") == "2"


def test_element_survives_store_destruction():
    """Element handles keep store data alive after the Store object is dropped."""
    def make_element():
        q = Query.all("a", Save.all()).build()
        store = parse('<a href="alive">x</a>', [q])
        return store.get("a")[0]

    el = make_element()
    assert el.name == "a"
    assert el.get_attribute("href") == "alive"
    assert el.text_content == "x"


def test_nested_element_survives_parent_list_destruction():
    """Child elements share the parent owner and remain usable after parents drop."""
    def make_child():
        q = Query.all("div", Save.all()).all("a", Save.all()).build()
        store = parse("<div><a href='nested'>n</a></div>", [q])
        parents = store.get("div")
        child = parents[0].get("a")[0]
        del parents
        del store
        return child

    child = make_child()
    assert child.name == "a"
    assert child.get_attribute("href") == "nested"


def test_selective_lookup_cardinality():
    """Selective queries return exact ordered matches without depending on store size."""
    html = "".join(
        ["<s1>x</s1>"]
        + ["<s10>x</s10>"] * 10
        + ["<a>x</a>"] * 100
    )
    store = parse(
        html,
        [
            Query.all("s1", Save.all()).build(),
            Query.all("s10", Save.all()).build(),
            Query.all("a", Save.all()).build(),
        ],
    )
    one = store.get("s1")
    ten = store.get("s10")
    all_a = store.get("a")
    assert one is not None and len(one) == 1
    assert ten is not None and len(ten) == 10
    assert all_a is not None and len(all_a) == 100
    assert one[0].name == "s1"
    assert ten[0].name == "s10"
    assert ten[-1].name == "s10"


# ── Exception mapping ──────────────────────────────────────────────────────


def test_build_invalid_selector_raises_value_error():
    with pytest.raises(ValueError):
        Query.all("!", Save.none()).build()


def test_parse_empty_queries_raises_value_error():
    with pytest.raises(ValueError, match="at least one query"):
        parse("<a></a>", [])


def test_element_get_missing_child_raises_value_error():
    q = Query.all("div", Save.all()).build()
    store = parse("<div></div>", [q])
    div = store.get("div")[0]
    with pytest.raises(ValueError, match="does not have children selected"):
        div.get("a")


def test_store_get_missing_returns_none():
    q = Query.all("a", Save.all()).build()
    store = parse("<div></div>", [q])
    assert store.get("missing") is None


def test_query_builder_does_not_expose_try_build():
    builder = Query.all("a", Save.none())
    assert not hasattr(builder, "try_build")


def test_attributes_preserve_missing_versus_empty():
    """Boolean attributes without values map to None; empty values map to ''."""
    q = Query.all("input", Save.all()).build()
    store = parse('<input disabled value="">', [q])
    element = store.get("input")[0]
    assert element.attributes["disabled"] is None
    assert element.attributes["value"] == ""


def test_large_result_lookup_correctness():
    """10_000-match lookup returns the correct ordered collection."""
    count = 10_000
    html = "".join(f'<a href="/{i}">x</a>' for i in range(count))
    q = Query.all("a", Save.all()).build()
    store = parse(html, [q])
    hits = store.get("a")
    assert hits is not None
    assert len(hits) == count
    assert hits[0].get_attribute("href") == "/0"
    assert hits[-1].get_attribute("href") == f"/{count - 1}"
    assert len(store) == count


def test_nested_lookup_and_public_types():
    q = (
        Query.all("div", Save.all())
        .all("span", Save.all())
        .build()
    )
    store = parse("<div><span id='s'>hi</span></div>", [q])
    divs = store.get("div")
    assert isinstance(divs, list)
    spans = divs[0].get("span")
    assert isinstance(spans, list)
    assert spans[0].id == "s"
    assert spans[0].name == "span"
