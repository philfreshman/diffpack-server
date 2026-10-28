use diffpack_server::page;

#[test]
fn the_response_ceiling_is_vercels_four_and_a_half_megabytes() {
    assert_eq!(
        page::RESPONSE_CEILING,
        4_500_000,
        "the cap Vercel puts on a function's response body"
    );
}

#[test]
fn a_payload_filled_to_the_ceiling_still_fits_in_the_response_that_carries_it() {
    let worst = "\"".repeat((page::PAYLOAD_CEILING - 2) / 2);
    let payload = serde_json::json!(worst);
    assert!(
        serde_json::to_vec(&payload)
            .expect("a string serialises")
            .len()
            >= page::PAYLOAD_CEILING - 2,
        "the test's own payload should be the ceiling it claims to test"
    );

    let answer = serde_json::json!({
        "items": [payload],
        "total": usize::MAX,
        "nextCursor": format!("p1:{}", usize::MAX),
    });
    let framed = serde_json::json!({
        "jsonrpc": "2.0",
        "id": u64::MAX,
        "result": rmcp::model::CallToolResult::structured(answer),
    });

    let bytes = serde_json::to_vec(&framed)
        .expect("a result serialises")
        .len();

    assert!(
        bytes <= page::RESPONSE_CEILING,
        "a payload at the ceiling framed to {bytes} bytes, over the {} the platform allows",
        page::RESPONSE_CEILING
    );
}

#[test]
fn a_page_holds_the_limit_and_states_the_total_rather_than_the_count() {
    let all: Vec<String> = (0..1_000).map(|n| format!("src/file{n}.rs")).collect();

    let page = page::paginate(all.clone(), None, None).expect("a page of strings");

    assert_eq!(
        page.items,
        all[..page::DEFAULT_LIMIT],
        "the default limit's worth, from the start"
    );
    assert_eq!(
        page.total, 1_000,
        "the length of the sequence, not of the page"
    );
    assert!(
        page.next_cursor.is_some(),
        "800 items are unreturned, so there is a next page to ask for"
    );
}

#[test]
fn walking_every_page_yields_each_item_exactly_once_and_in_order() {
    let all: Vec<String> = (0..1_003).map(|n| format!("src/file{n}.rs")).collect();

    let mut seen: Vec<String> = Vec::new();
    let mut cursor: Option<page::Cursor> = None;
    let mut pages = 0;

    loop {
        let page = page::paginate(all.clone(), Some(page::Limit::new(37)), cursor)
            .expect("every cursor resumes");

        assert_eq!(
            page.total,
            all.len(),
            "the total is the sequence's, on every page and not only the first"
        );

        pages += 1;
        assert!(
            pages <= 28,
            "a cursor that does not advance would walk forever"
        );

        seen.extend(page.items);
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }

    assert_eq!(
        pages, 28,
        "1003 items in pages of 37 is 27 full pages and a short one"
    );
    assert_eq!(
        seen, all,
        "the walk is the sequence: no duplicate, no gap, no reordering"
    );
}

fn serialised_bytes<T: serde::Serialize>(items: &[T]) -> usize {
    items
        .iter()
        .map(|item| serde_json::to_vec(item).expect("an item serialises").len())
        .sum()
}

#[test]
fn the_ceiling_is_on_bytes_so_one_limit_is_not_safe_for_every_item() {
    let small: Vec<String> = (0..200).map(|n| format!("src/file{n}.rs")).collect();
    let large: Vec<String> = (0..200)
        .map(|n| format!("line {n}\n").repeat(12_000))
        .collect();

    let by_count = page::paginate(small.clone(), Some(page::Limit::new(200)), None)
        .expect("a page of small items");
    assert_eq!(
        by_count.items.len(),
        200,
        "200 short paths are nowhere near the ceiling, so the limit is what binds"
    );

    let by_bytes = page::paginate(large.clone(), Some(page::Limit::new(200)), None)
        .expect("a page of large items");
    assert!(
        by_bytes.items.len() < 200,
        "200 items of ~90 KB is over 17 MB; the limit cannot be what binds"
    );
    assert!(
        !by_bytes.items.is_empty(),
        "each item fits on its own, so a page must carry at least one"
    );
    assert!(
        serialised_bytes(&by_bytes.items) <= page::PAYLOAD_CEILING,
        "a page cut by bytes is a page under the ceiling"
    );
    assert_eq!(
        by_bytes.total, 200,
        "the total is the sequence's length however few of it fitted"
    );
    assert!(
        by_bytes.next_cursor.is_some(),
        "a page cut short by bytes has more to come, and must say where"
    );
}

#[test]
fn a_walk_is_stable_when_bytes_rather_than_the_limit_end_each_page() {
    let all: Vec<String> = (0..120)
        .map(|n: usize| "x".repeat(1 + (n * 7_919) % 400_000))
        .collect();

    let mut seen: Vec<String> = Vec::new();
    let mut cursor: Option<page::Cursor> = None;
    let mut cut_by_bytes = 0;
    let mut pages = 0;

    loop {
        let page = page::paginate(
            all.clone(),
            Some(page::Limit::new(page::MAX_LIMIT as u32)),
            cursor,
        )
        .expect("every cursor resumes");

        pages += 1;
        assert!(
            pages <= all.len(),
            "a page that takes nothing would walk forever"
        );
        assert!(
            serialised_bytes(&page.items) <= page::PAYLOAD_CEILING,
            "no page is over the ceiling, whatever mix of sizes landed on it"
        );

        if page.next_cursor.is_some() {
            cut_by_bytes += 1;
        }

        seen.extend(page.items);
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }

    assert!(
        cut_by_bytes > 0,
        "the limit was the sequence's whole length, so only bytes can have ended a page"
    );
    assert_eq!(
        seen, all,
        "the walk is the sequence: no duplicate, no gap, no reordering"
    );
}

#[test]
fn an_item_too_large_for_a_page_of_its_own_is_refused_rather_than_dropped() {
    let huge = "x".repeat(page::PAYLOAD_CEILING);
    let all = vec!["before".to_owned(), huge, "after".to_owned()];

    let first =
        page::paginate(all.clone(), Some(page::Limit::new(10)), None).expect("the first page");
    assert_eq!(
        first.items,
        ["before"],
        "the oversized item does not fit beside the one before it"
    );
    assert_eq!(first.next_cursor, Some(page::Cursor::at(1)));

    let refused = page::paginate(all.clone(), Some(page::Limit::new(10)), first.next_cursor)
        .expect_err("an item over the ceiling cannot be returned in any page");
    let said = message_of(refused);

    assert!(
        said.contains(&(page::PAYLOAD_CEILING + 2).to_string()),
        "the failure states the item's real size; it said: {said}"
    );
    assert!(
        said.contains("p1:2"),
        "the failure carries the cursor that continues past it; it said: {said}"
    );

    let past = page::paginate(
        all.clone(),
        Some(page::Limit::new(10)),
        Some(page::Cursor::at(2)),
    )
    .expect("a page past it");
    assert_eq!(
        past.items,
        ["after"],
        "and that cursor really does continue"
    );
    assert_eq!(past.total, 3, "with the whole sequence still counted");
}

fn message_of(failure: diffpack_server::error::Failure) -> String {
    match failure.respond() {
        Ok(result) => result
            .content
            .iter()
            .filter_map(|block| block.as_text().map(|text| text.text.clone()))
            .collect::<Vec<_>>()
            .join("\n"),
        Err(error) => error.message.into_owned(),
    }
}

#[test]
fn a_cursor_that_is_not_ours_is_refused_on_the_protocol_channel() {
    let all: Vec<u32> = (0..10).collect();

    for wrong in [
        "", "5", "p2:5", "p1:", "p1:five", "p1:-1", "cursor", "p1:5:5",
    ] {
        let refused = page::Cursor::decode(wrong)
            .err()
            .unwrap_or_else(|| panic!("`{wrong}` is not a cursor this module wrote"));

        assert!(
            refused.respond().is_err(),
            "`{wrong}` is the client's mistake, so it belongs where the client reads"
        );
    }

    let page =
        page::paginate(all.clone(), Some(page::Limit::new(4)), None).expect("the first page");
    assert_eq!(page.next_cursor, Some(page::Cursor::at(4)));
    let second = page::paginate(all.clone(), Some(page::Limit::new(4)), page.next_cursor)
        .expect("the cursor it wrote");
    assert_eq!(second.items, [4, 5, 6, 7]);
}

#[test]
fn limit_is_clamped_into_the_documented_range() {
    let all: Vec<u32> = (0..5_000).collect();

    assert_eq!(
        page::paginate(all.clone(), None, None)
            .expect("a page")
            .items
            .len(),
        page::DEFAULT_LIMIT,
        "no limit means the documented default"
    );
    assert_eq!(
        page::paginate(all.clone(), Some(page::Limit::new(99_999)), None)
            .expect("a page")
            .items
            .len(),
        page::MAX_LIMIT,
        "more than the maximum means the maximum"
    );
    assert_eq!(
        page::paginate(all.clone(), Some(page::Limit::new(0)), None)
            .expect("a page")
            .items
            .len(),
        1,
        "a page of nothing is not a page, and a walk made of them would not advance"
    );
}

#[test]
fn a_cursor_past_the_end_ends_the_walk_rather_than_failing_it() {
    let all: Vec<u32> = (0..10).collect();

    let page = page::paginate(all, None, Some(page::Cursor::at(10))).expect("a page at the end");

    assert!(page.items.is_empty());
    assert_eq!(page.total, 10, "the sequence is still ten long");
    assert_eq!(
        page.next_cursor, None,
        "and there is nothing after it to ask for"
    );
}

#[test]
fn a_blob_under_the_ceiling_comes_back_whole() {
    let content = "fn main() {\n    println!(\"hello\");\n}\n";

    let excerpt = page::truncate(content, page::MaxBytes::CEILING);

    assert_eq!(
        excerpt.text, content,
        "nothing was cut, so nothing was added"
    );
    assert!(!excerpt.truncated);
    assert_eq!(excerpt.bytes, content.len());
}

#[test]
fn a_blob_over_the_ceiling_is_cut_and_says_so_in_the_text() {
    let content = "日本語のコード\n".repeat(80_000);
    assert!(
        content.len() > page::PAYLOAD_CEILING,
        "the test's own input must not fit"
    );

    let excerpt = page::truncate(&content, page::MaxBytes::CEILING);

    assert!(excerpt.truncated, "it did not fit, so it was cut");
    assert_eq!(
        excerpt.bytes,
        content.len(),
        "the real size of the whole thing, not of what came back"
    );

    let marker = excerpt
        .text
        .find("[truncated")
        .expect("an explicit marker, in the text a model reads");
    assert!(
        content.starts_with(&excerpt.text[..marker]),
        "what was shown is a prefix of what there was, cut on a character boundary"
    );
    assert!(
        excerpt.text[marker..].contains(&content.len().to_string()),
        "the marker states the real total: {}",
        &excerpt.text[marker..]
    );
    assert!(
        serde_json::to_vec(&excerpt.text)
            .expect("text serialises")
            .len()
            <= page::PAYLOAD_CEILING,
        "a cut excerpt is an excerpt under the ceiling"
    );
}

#[test]
fn a_cut_blob_is_cut_on_what_it_costs_encoded_not_on_its_own_length() {
    let worst = page::truncate(&"\"".repeat(4_000_000), page::MaxBytes::CEILING);
    let plain = page::truncate(&"x".repeat(4_000_000), page::MaxBytes::CEILING);

    let shown = |excerpt: &page::Excerpt| {
        excerpt
            .text
            .find("\n[truncated")
            .expect("a cut blob carries its marker")
    };

    assert!(worst.truncated && plain.truncated);
    assert_eq!(
        worst.bytes, plain.bytes,
        "the two blobs are the same length before anything is cut"
    );
    assert_eq!(
        shown(&worst) * 2,
        shown(&plain),
        "a character that escapes to two bytes costs two bytes of the ceiling"
    );

    let framed = serde_json::json!({
        "jsonrpc": "2.0",
        "id": u64::MAX,
        "result": rmcp::model::CallToolResult::structured(
            serde_json::to_value(&worst).expect("an excerpt serialises"),
        ),
    });
    let bytes = serde_json::to_vec(&framed)
        .expect("a result serialises")
        .len();

    assert!(
        bytes <= page::RESPONSE_CEILING,
        "the worst excerpt there is framed to {bytes} bytes, over the {} allowed",
        page::RESPONSE_CEILING
    );
}

#[test]
fn max_bytes_narrows_the_cut_and_cannot_widen_it() {
    let content = "abcdefghij".repeat(1_000);

    let asked_for_less = page::truncate(&content, page::MaxBytes::new(100));
    let shown = asked_for_less
        .text
        .find("\n[truncated")
        .expect("a cut blob carries its marker");
    assert_eq!(
        shown, 100,
        "the cap the caller asked for, in the text's own bytes"
    );
    assert!(asked_for_less.truncated);
    assert_eq!(asked_for_less.bytes, 10_000, "and still the real total");

    let asked_for_everything = page::truncate(&content, page::MaxBytes::new(u64::MAX));
    assert_eq!(
        asked_for_everything.text, content,
        "a cap larger than the text is not a cut"
    );
    assert!(!asked_for_everything.truncated);

    let huge = "x".repeat(page::PAYLOAD_CEILING * 2);
    let refused_the_raise = page::truncate(&huge, page::MaxBytes::new(u64::MAX));
    assert!(
        refused_the_raise.truncated,
        "asking for all of it does not make it fit"
    );
    assert!(
        serde_json::to_vec(&refused_the_raise.text)
            .expect("text serialises")
            .len()
            <= page::PAYLOAD_CEILING
    );
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[schemars(
    description = "The pagination arguments as a tool declares them. Written out here rather\n\
    than reached for inside `src/tools/` because it is the shape being tested,\n\
    not any one tool: whatever #11 and #14 call themselves, this is what their\n\
    `Args` has to contain for the numbers to be inherited rather than retyped."
)]
#[serde(deny_unknown_fields)]
struct PagingArgs {
    #[serde(default)]
    limit: Option<page::Limit>,
    #[serde(default)]
    cursor: Option<page::Cursor>,
}

#[test]
fn a_paginating_tools_schema_inherits_the_default_and_the_maximum() {
    let schema = serde_json::to_value(schemars::schema_for!(PagingArgs)).expect("a schema");
    let limit = &schema["properties"]["limit"];

    assert_eq!(
        limit["maximum"],
        serde_json::json!(page::MAX_LIMIT),
        "the maximum an agent is shown is the one that binds, got {limit}"
    );
    assert_eq!(
        limit["default"],
        serde_json::json!(page::DEFAULT_LIMIT),
        "and so is the default it gets by omitting the field, got {limit}"
    );
    assert_eq!(
        limit["minimum"],
        serde_json::json!(1),
        "a page of nothing is not a page, got {limit}"
    );
    assert!(
        limit["description"]
            .as_str()
            .is_some_and(|said| said.contains("clamp")),
        "an agent that asks for too much is not refused, and the schema says so, got {limit}"
    );
}

#[test]
fn the_cursor_a_tool_declares_says_to_pass_it_back_unchanged() {
    let schema = serde_json::to_value(schemars::schema_for!(PagingArgs)).expect("a schema");
    let cursor = &schema["properties"]["cursor"];

    assert_eq!(
        cursor["type"],
        serde_json::json!(["string", "null"]),
        "a cursor crosses the wire as the string this module wrote, got {cursor}"
    );
    assert_eq!(
        cursor["pattern"], "^p1:[0-9]+$",
        "the one format, stated where a client reads it, got {cursor}"
    );
    assert!(
        cursor["description"]
            .as_str()
            .is_some_and(|said| said.contains("unchanged")),
        "the rule is that it is passed back as it arrived, got {cursor}"
    );

    assert!(
        cursor["description"]
            .as_str()
            .is_some_and(|said| said.contains("nextCursor") && !said.contains("next_cursor")),
        "the description names the field as a page spells it, got {cursor}"
    );
}

#[test]
fn a_cursor_that_is_not_ours_is_refused_before_a_handler_runs() {
    let refused = serde_json::from_value::<PagingArgs>(serde_json::json!({ "cursor": "5" }))
        .expect_err("a bare offset is not a cursor this module wrote");

    assert!(
        refused.to_string().contains("nextCursor"),
        "the refusal says what to pass instead, by the name a page gives it; \
         it said: {refused}"
    );

    let accepted = serde_json::from_value::<PagingArgs>(serde_json::json!({ "cursor": "p1:4" }))
        .expect("the cursor this module writes is the cursor it reads");
    assert_eq!(accepted.cursor, Some(page::Cursor::at(4)));
}

#[test]
fn a_limit_is_a_number_on_the_wire_and_the_module_clamps_it() {
    let args = serde_json::from_value::<PagingArgs>(serde_json::json!({ "limit": 99_999 }))
        .expect("a limit outside the range is clamped rather than refused");

    let all: Vec<u32> = (0..5_000).collect();
    assert_eq!(
        page::paginate(all, args.limit, args.cursor)
            .expect("a page")
            .items
            .len(),
        page::MAX_LIMIT,
        "the clamp is the module's, wherever the number came from"
    );
}
