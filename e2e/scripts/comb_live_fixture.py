"""Fictional facts with unpredictable answers, and a credential-free socket audit."""

import uuid


CASES = [
    (
        "The hillside observatory keeps its telescope tracking during electrical outages using a flywheel energy store.",
        "How does the astronomical station maintain its instrument's movement when grid electricity fails?",
    ),
    (
        "The paper archive suppresses fires by releasing nitrogen rather than spraying water onto its manuscripts.",
        "Which fire protection method preserves the historical documents without soaking them?",
    ),
    (
        "The apricot orchard protects blossoms from overnight frost with overhead irrigation that forms an insulating ice layer.",
        "How are the fruit trees' flowers saved when the temperature drops below freezing at night?",
    ),
    (
        "The submarine telecommunications cable team locates damaged optical fiber by timing reflected laser pulses.",
        "How do technicians pinpoint a broken section of the undersea data connection?",
    ),
    (
        "The museum keeps ancient bronze exhibits dry by putting regenerable molecular-sieve cartridges inside display cases.",
        "What removes excess moisture around the antiquities so that the metal does not corrode?",
    ),
    (
        "The remote clinic stores vaccines in a solar-powered refrigerator with phase-change cold packs for cloudy days.",
        "How does the isolated health center keep its immunization supplies chilled without reliable mains power?",
    ),
    (
        "The mountain shelter makes collected rainwater safe to drink with a ceramic filter followed by ultraviolet disinfection.",
        "What treatment makes roof runoff potable for hikers staying at the alpine refuge?",
    ),
    (
        "The tomato greenhouse pollinates flowers using vibration from a small handheld electric wand instead of insects.",
        "How does the enclosed vegetable farm transfer pollen when there are no bees?",
    ),
    (
        "The district swimming pool heats its water with waste heat recovered from a neighboring data center through a heat exchanger.",
        "Where does the public aquatic facility obtain thermal energy that would otherwise be thrown away by computer equipment?",
    ),
    (
        "The desert wildlife station counts passing animals at night using passive infrared trail cameras with no visible flash.",
        "How do researchers monitor nocturnal creatures without lighting up their habitat?",
    ),
]


def make_facts():
    return [
        dict(
            id=f"Q{i:02}",
            slug=f"facts/record-{uuid.uuid4().hex[:12]}",
            marker=f"HXFACT{uuid.uuid4().hex[:16]}",
            code=f"VALUE{uuid.uuid4().hex}",
            fact=fact,
            query=query,
        )
        for i, (fact, query) in enumerate(CASES, 1)
    ]


def verify_corrected_fact(text):
    # Check the changed operational claims, without requiring incidental prose
    # such as "now". The random code independently proves new-source retrieval.
    for claim in ("using a hydrogen fuel cell", "The flywheel energy store was retired."):
        if claim not in text:
            raise AssertionError("Corrected operational fact is missing or stale")


def verify_consolidation(before, after, marked, original, corrected, curator):
    page = after["page"]
    if page["timeline"][1:] != before["page"]["timeline"] or len(page["timeline"]) != 2:
        raise AssertionError("Curator rewrote prior evidence or failed to add the correction")
    verify_corrected_fact(page["compiled"])
    if corrected["code"] not in page["compiled"] or original["code"] in page["compiled"]:
        raise AssertionError("Curator did not replace the stale verification code")
    copied = page["timeline"][0]["metadata"]
    if (
        copied["actor"].get("id") != curator
        or copied["consolidation"]["source"] != marked["slug"]
        or not after["unverified"]
    ):
        raise AssertionError("Consolidation attribution or trust is incorrect")
    checkpoints = marked["page"]["metadata"]["consolidated_into"]
    if len(checkpoints) != 1 or checkpoints[0]["target"] != original["slug"]:
        raise AssertionError("Inbox consolidation checkpoint is missing")


def verify_relearning(fact, agent_id, rows, answer):
    """Two fresh searches and a fresh canonical source for both corrected answers.

    A single get after both searches suffices when both retrieve the same page.
    An inbox read alone or a source read predating either search does not.
    """
    calls = [row for row in rows if (row.get("claim") or {}).get("id") == agent_id]
    if sum(row["operation"]["op"] == "search" for row in calls) != 2:
        raise AssertionError("Expected exactly two new searches after correction")
    checks = []
    for mode in ("vector", "text"):
        selected = [
            row
            for row in calls
            if row["operation"]["op"] != "search" or row["operation"]["mode"] == mode
        ]
        checks += verify_retrieval([fact], agent_id, selected, answer[mode], mode, text_max_rank=3)
        verify_corrected_fact(answer[mode][0].get("fact", ""))
    return checks


def verify_retrieval(facts, agent_id, rows, answers, mode, *, text_max_rank=1):
    """Reject guessed answers, wrong modes, fallback, missing reads and broad scans."""
    if not isinstance(answers, list) or len(answers) != len(facts):
        raise AssertionError(f"Expected exactly {len(facts)} reader answers")
    by_id = {answer["id"]: answer for answer in answers}
    if set(by_id) != {fact["id"] for fact in facts}:
        raise AssertionError("Reader answer IDs differ from the assigned queries")
    calls = [row for row in rows if (row.get("claim") or {}).get("id") == agent_id]
    if any(row["operation"]["op"] not in ("search", "get") for row in calls):
        raise AssertionError("Reader used an operation other than search/get")
    checks = []
    for fact in facts:
        query = fact["query"] if mode == "vector" else fact["marker"]
        matching = [
            row
            for row in calls
            if row["operation"]["op"] == "search"
            and row["operation"]["mode"] == mode
            and row["operation"]["request"]["query"] == query
        ]
        if len(matching) != 1:
            raise AssertionError(f"{fact['id']}: expected one actual {mode} search")
        row = matching[0]
        request = row["operation"]["request"]
        if request["limit"] != 3 or any(
            request.get(key) for key in ("kind", "tag", "prefix", "after", "before")
        ):
            raise AssertionError("Reader narrowed or broadened the prescribed search")
        if row["reply"]["status"] != "ok":
            raise AssertionError(f"{fact['id']}: search failed")
        result = row["reply"]["result"]
        if result["mode"] != mode or result["fallback"] is not None:
            raise AssertionError("Search mode changed or fell back")
        hits = result["results"]
        rank = next((i for i, hit in enumerate(hits, 1) if hit["slug"] == fact["slug"]), None)
        if rank is None or rank > 3 or (mode == "text" and rank > text_max_rank):
            raise AssertionError(f"{fact['id']}: expected source was not retrieved")
        if mode == "vector" and (result["coverage"]["pending"] or result["coverage"]["rejected"]):
            raise AssertionError("Incomplete embedding coverage")
        reads = [
            read
            for read in calls
            if read["operation"] == dict(op="get", slug=fact["slug"])
            and read["reply"]["status"] == "ok"
            and read["start"] >= row["end"]
        ]
        if not reads:
            raise AssertionError(f"{fact['id']}: did not inspect the retrieved source")
        answer = by_id[fact["id"]]
        if answer.get("slug") != fact["slug"] or answer.get("verification_code") != fact["code"]:
            raise AssertionError(f"{fact['id']}: wrong recovered fact/code")
        checks.append(
            dict(
                id=fact["id"],
                mode=mode,
                rank=rank,
                slug=fact["slug"],
                query=query,
                search_ms=(row["end"] - row["start"]) * 1000,
            )
        )
    if sum(row["operation"]["op"] == "search" for row in calls) != len(facts):
        raise AssertionError("Unexpected additional searches")
    return checks
