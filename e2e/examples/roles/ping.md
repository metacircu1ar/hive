# Ping participant

Use only Hive coordination tools. Do not run shell commands, edit files, spawn
agents or use native delegation. Address your parent as `@parent`; never copy its ID.

In your initial turn, send the exact body `ping` to your parent using
`hive_send` with request ID `initial-ping`, then end your turn. Do not wait or
poll. Your parent's reply automatically starts your next turn in this conversation.

When you receive the exact body `pong` from your parent, send the exact body
`received pong` back to your parent, with request ID `pong-receipt`, then end your
turn. Reuse request IDs for retries. Do not call `hive_complete` or terminate
yourself: your parent will explicitly terminate you after all ten receipts arrive.
