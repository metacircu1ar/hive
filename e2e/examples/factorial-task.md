Exercise a real implementor/reviewer loop in this new temporary repository.

Acceptance contract for `factorial.py`:
- Expose `factorial(n)` for Python integers, excluding bool.
- Return exact n! for nonnegative integers, including 0! = 1.
- Raise ValueError for negative integers and TypeError for bool and all non-integers.
- Include Python standard-library unittest tests; no dependencies or network needed.

For this deliberate fault-injection demonstration, FIRST write an incorrect implementation
with an off-by-one multiplication range. Preserve an exact copy as
`evidence/initial_factorial.py` and its SHA-256 hash as `evidence/initial.sha256`.
Do not modify those evidence files or fix factorial.py before the first review.
Then launch ONE reviewer with `hive_spawn`, role `reviewer`, in this same repository.
Give it the contract, the file path, and the initial snapshot hash. Ask it to inspect the
actual implementation and return concrete findings to you through Hive. End your turn.

When the reviewer responds, fix its findings, add meaningful tests, and run them. Send the
same reviewer another review request using `hive_send` and end your turn. Repeat as
needed until that reviewer sends NO_FINDINGS. Retain both the faulty initial snapshot and
the corrected files so the sequence is auditable. The evidence snapshot intentionally
remains faulty and is not production code to correct. The reviewer must not edit files.

After a clean review and passing tests, call `hive_complete` with the initial hash,
reviewer ID, final files and test evidence. Do not use native provider delegation. Use only
Hive agent tools; do not implement a local scripted reviewer or make up findings.
Git metadata writes are unnecessary; leave .git unchanged.
