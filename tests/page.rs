//! The response ceiling, and what fits under it.
//!
//! Every expectation here comes from the platform's documented cap or from
//! the issue that owns the fact — #43 for the ceiling and the cursor, #11 and
//! #14 for stable pagination, #12 and #15 for truncation — and never from
//! running the code and recording what it said. The whole module is pure, so
//! nothing here needs a fixture: a generated sequence is a better witness
//! than a package that happened to be large on the day.

use diffpack_server::page;

/// Vercel caps a function's response body at 4.5 MB. The number is a
/// platform fact rather than a preference, so it is written here from the
/// documentation rather than read back out of the module it constrains.
#[test]
fn the_response_ceiling_is_vercels_four_and_a_half_megabytes() {
    assert_eq!(
        page::RESPONSE_CEILING,
        4_500_000,
        "the cap Vercel puts on a function's response body"
    );
}

/// The ceiling is on the whole response, and a tool's answer is not the whole
/// response: `tools::invoke` hands it to `CallToolResult::structured`, which
/// puts it on the wire twice — once as `structuredContent` and once as the
/// text block rmcp mirrors it into, JSON-escaped. A payload filled to
/// [`page::PAYLOAD_CEILING`] with the worst content there is — every byte a
/// character that escaping doubles — still has to fit, wrapped and framed.
#[test]
fn a_payload_filled_to_the_ceiling_still_fits_in_the_response_that_carries_it() {
    // `"` serialises as `\"`, so a string of them is the most expensive
    // payload a tool can produce: nothing inflates further.
    let worst = "\"".repeat((page::PAYLOAD_CEILING - 2) / 2);
    let payload = serde_json::json!(worst);
    assert!(
        serde_json::to_vec(&payload)
            .expect("a string serialises")
            .len()
            >= page::PAYLOAD_CEILING - 2,
        "the test's own payload should be the ceiling it claims to test"
    );

    // A Page's envelope around it, with the widest cursor and total it could
    // carry, and then the JSON-RPC frame the transport writes.
    let answer = serde_json::json!({
        "items": [payload],
        "total": usize::MAX,
        "next_cursor": format!("p1:{}", usize::MAX),
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

/// A sequence longer than one page comes back cut to the limit, carrying the
/// real total rather than the returned count and a cursor for the rest. The
/// total is the honesty requirement: an agent told it received 200 of 200 has
/// no reason to ask for more.
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

/// The property #11 and #14 each asked for in the same words: walking every
/// page yields each item exactly once, with no duplicates and nothing
/// skipped. Asserting the concatenation *equals* the sequence proves all
/// three at once — a duplicate, a gap or a reordering each break the
/// equality — and proves it here rather than five times against five
/// fixtures.
#[test]
fn walking_every_page_yields_each_item_exactly_once_and_in_order() {
    // A length that is not a multiple of the limit, so the last page is a
    // short one rather than an exact fit.
    let all: Vec<String> = (0..1_003).map(|n| format!("src/file{n}.rs")).collect();

    let mut seen: Vec<String> = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;

    loop {
        let page =
            page::paginate(all.clone(), Some(37), cursor.as_deref()).expect("every cursor resumes");

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

/// How many bytes `items` occupy once serialised, which is the only measure
/// the ceiling is in.
fn serialised_bytes<T: serde::Serialize>(items: &[T]) -> usize {
    items
        .iter()
        .map(|item| serde_json::to_vec(item).expect("an item serialises").len())
        .sum()
}

/// The trap five separate implementations would each have had to notice: the
/// cap is on serialised bytes, so a `limit` that is safe for a tree entry is
/// not safe for a file listing. The same limit over the same number of items
/// returns all of them when they are small and fewer when they are large,
/// and the module — not the caller — is what noticed.
#[test]
fn the_ceiling_is_on_bytes_so_one_limit_is_not_safe_for_every_item() {
    let small: Vec<String> = (0..200).map(|n| format!("src/file{n}.rs")).collect();
    let large: Vec<String> = (0..200)
        .map(|n| format!("line {n}\n").repeat(12_000))
        .collect();

    let by_count = page::paginate(small.clone(), Some(200), None).expect("a page of small items");
    assert_eq!(
        by_count.items.len(),
        200,
        "200 short paths are nowhere near the ceiling, so the limit is what binds"
    );

    let by_bytes = page::paginate(large.clone(), Some(200), None).expect("a page of large items");
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

/// Stable pagination has to survive the page being cut by bytes rather than
/// by the limit, which is the case a fixture of uniform items never reaches.
/// The sizes here vary by four orders of magnitude inside one sequence, so
/// no page is the same length as another and the cursor cannot be riding on
/// the limit by accident.
#[test]
fn a_walk_is_stable_when_bytes_rather_than_the_limit_end_each_page() {
    let all: Vec<String> = (0..120)
        .map(|n: usize| "x".repeat(1 + (n * 7_919) % 400_000))
        .collect();

    let mut seen: Vec<String> = Vec::new();
    let mut cursor: Option<String> = None;
    let mut cut_by_bytes = 0;
    let mut pages = 0;

    loop {
        let page = page::paginate(all.clone(), Some(page::MAX_LIMIT as u32), cursor.as_deref())
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

/// An item larger than a whole page is the case that has to be loud. Dropping
/// it would leave a walk that says it covered the sequence and did not, which
/// is the failure an agent cannot detect and cannot recover from.
///
/// So it is refused — and the refusal carries the cursor that continues past
/// it, which is what keeps "loud" from meaning "stuck": skipping the item
/// becomes something the agent chose rather than something this module did
/// behind its back.
#[test]
fn an_item_too_large_for_a_page_of_its_own_is_refused_rather_than_dropped() {
    let huge = "x".repeat(page::PAYLOAD_CEILING);
    let all = vec!["before".to_owned(), huge, "after".to_owned()];

    let first = page::paginate(all.clone(), Some(10), None).expect("the first page");
    assert_eq!(
        first.items,
        ["before"],
        "the oversized item does not fit beside the one before it"
    );
    assert_eq!(first.next_cursor.as_deref(), Some("p1:1"));

    let refused = page::paginate(all.clone(), Some(10), first.next_cursor.as_deref())
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

    let past = page::paginate(all.clone(), Some(10), Some("p1:2")).expect("a page past it");
    assert_eq!(
        past.items,
        ["after"],
        "and that cursor really does continue"
    );
    assert_eq!(past.total, 3, "with the whole sequence still counted");
}

/// The text a model would read for `failure`, whichever channel it takes.
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

/// One cursor format across every paginating tool, and it is this one. A
/// client is told to treat a cursor as opaque, so a cursor that does not
/// decode is the client's mistake rather than something a model can fix by
/// rewording — it takes the protocol channel, where the client reads.
#[test]
fn a_cursor_that_is_not_ours_is_refused_on_the_protocol_channel() {
    let all: Vec<u32> = (0..10).collect();

    // A bare offset, a future format's cursor, a truncated one, and junk.
    for wrong in [
        "", "5", "p2:5", "p1:", "p1:five", "p1:-1", "cursor", "p1:5:5",
    ] {
        let refused = page::paginate(all.clone(), None, Some(wrong))
            .err()
            .unwrap_or_else(|| panic!("`{wrong}` is not a cursor this module wrote"));

        assert!(
            refused.respond().is_err(),
            "`{wrong}` is the client's mistake, so it belongs where the client reads"
        );
    }

    // And the one that is ours round-trips, which is what makes it a format
    // rather than an implementation detail two tools could spell differently.
    let page = page::paginate(all.clone(), Some(4), None).expect("the first page");
    assert_eq!(page.next_cursor.as_deref(), Some("p1:4"));
    let second = page::paginate(all.clone(), Some(4), page.next_cursor.as_deref())
        .expect("the cursor it wrote");
    assert_eq!(second.items, [4, 5, 6, 7]);
}

/// #23 asks that every optional argument document its default and every
/// constrained one its range. Both live here rather than in each tool's
/// schema prose, so that the numbers a tool writes down are the numbers that
/// bind.
///
/// A `limit` outside the range is clamped rather than refused: a client that
/// asked for too much still gets an answer, and the page states the total, so
/// the clamp is visible rather than silent.
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
        page::paginate(all.clone(), Some(99_999), None)
            .expect("a page")
            .items
            .len(),
        page::MAX_LIMIT,
        "more than the maximum means the maximum"
    );
    assert_eq!(
        page::paginate(all.clone(), Some(0), None)
            .expect("a page")
            .items
            .len(),
        1,
        "a page of nothing is not a page, and a walk made of them would not advance"
    );
}

/// A cursor at or past the end is the natural end of a walk, not a failure:
/// it is what a client holds after reading the last page of a sequence that
/// has since been recomputed shorter. It answers empty, says the total, and
/// does not hand out another cursor.
#[test]
fn a_cursor_past_the_end_ends_the_walk_rather_than_failing_it() {
    let all: Vec<u32> = (0..10).collect();

    let page = page::paginate(all, None, Some("p1:10")).expect("a page at the end");

    assert!(page.items.is_empty());
    assert_eq!(page.total, 10, "the sequence is still ten long");
    assert_eq!(
        page.next_cursor, None,
        "and there is nothing after it to ask for"
    );
}
