pub(super) const RECOVER_AGENTS: &str = "\nUPDATE redirect_interrupts\nSET state = \
    'target_lost'\nWHERE state IN ('pending', 'sent');\nUPDATE messages\nSET state = \
    'stale_turn'\nWHERE mode = 'steer'\n    AND state = 'accepted';\nUPDATE messages\nSET \
    state = 'uncertain'\nWHERE state IN ('submitting', 'submitted');\nUPDATE agents\nSET \
    state = 'stopped', turn_id = NULL, control = NULL, completion_pending = NULL, claimed = \
    0,\n    blocked = 0, data = json_set(json_remove(data, '$.steering_supported'), \
    '$.turn_selection',\n        NULL, '$.completion_pending_by', NULL, '$.redirect_pending', json('false'));\n";

pub const STALLED_AGENTS: &str = "\nSELECT a.id, a.parent_id, json_extract(a.data, '$.role'), \
    a.paused, (a.state = 'running'\n        OR a.turn_id IS NOT NULL\n        OR EXISTS (\n        \
    SELECT 1\n        FROM messages\n        WHERE recipient = a.id\n            AND mode = \
    'next-turn'\n            AND state IN ('submitting', 'submitted'))), (a.pending_count > \
    0\n        OR a.waiting = 1\n        OR EXISTS (SELECT 1 FROM joins WHERE requester = \
    a.id AND state IN ('collecting', 'deadline_passed'))\n        OR a.branch_hold > 0\n        \
    OR (a.branch_gate IS NOT NULL\n            AND a.branch_gate != 'bound')\n        OR \
    EXISTS (\n        SELECT 1\n        FROM approvals\n        WHERE agent = a.id\n            \
    AND state = 'pending')\n        OR EXISTS (\n        SELECT 1\n        FROM messages\n        \
    WHERE recipient = a.id\n            AND state = 'scheduled')\n        OR EXISTS (\n        \
    SELECT 1\n        FROM messages\n        WHERE recipient = a.id\n            AND mode = \
    'steer'\n            AND state = 'accepted')), json_extract(a.data, '$.name')\nFROM \
    agents a\nWHERE a.task_outcome = 'open'\n    AND a.expired = 0\n";
