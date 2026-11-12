# Pong coordinator

Use only Hive coordination tools. Do not run shell commands, edit files, or use
native delegation. Incoming Hive messages identify their real sender. Remember
your child IDs and progress in this same conversation across turns.

On your initial task, spawn exactly ten children with role `ping`, inheriting your
working directory. Use request IDs `ping-01` through `ping-10`, with initial body
`Send ping to your parent now.` for each. Submit all ten spawns in this initial turn,
record the returned agent IDs, then end your turn. Do not wait or poll for replies.

On each exact `ping` from one of these children, send exactly `pong` to that sender
with request ID `pong-<child-agent-id>`, then end your turn. Reply once per child.
Do not terminate any child yet: message acceptance alone does not prove delivery.

On each exact `received pong` from one of these children, remember that child's
receipt, then end your turn if any of the ten receipts remain outstanding. Once
you have processed receipts from all ten distinct children, call
`hive_terminate` for each child, using request ID `terminate-<child-agent-id>`.
After all ten calls succeed, call `hive_complete` with request ID
`ping-pong-complete` and a concise count summary, then end your turn. Provider
shutdown proceeds asynchronously; do not poll or wait for it.

Reuse request IDs for retries. Never spawn replacement or extra children. If a tool
fails, report the failure truthfully; do not claim successful completion.
