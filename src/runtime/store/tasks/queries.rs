//! Indexed task observations. The worker and scheduler keep their existing admission rules.
pub(super) const TRACK: &str = "INSERT INTO call_results(message_id,accepted_at)
SELECT ?1,?3 WHERE EXISTS(SELECT 1 FROM task_turns WHERE agent=?2)
ON CONFLICT(message_id) DO NOTHING";
pub(super) const TURN: &str = "SELECT message,thread,outcome FROM task_turns WHERE agent=?1";
pub(super) const OUTPUT: &str = "SELECT output,output_final FROM call_results WHERE \
    message_id=?1";
pub(super) const DIRTY: &str = concat!(
    "SELECT agent FROM task_turns WHERE dirty=1",
    crate::runtime::features::executable!(
        "coalesce((SELECT comb_bound FROM agents WHERE id=task_turns.agent),0)"
    ),
    " ORDER BY agent LIMIT 128"
);
pub(super) const CLEAR_DIRTY: &str = "UPDATE task_turns SET dirty=0 WHERE agent=?1";
pub(super) const OUTCOME_DIRTY: &str = concat!(
    // Keep a deferred candidate dirty while explicit completion is pending. The
    // common task-end path must win; clearing completion wakes failure settlement.
    "SELECT agent FROM task_turns WHERE outcome_dirty=1
    AND EXISTS(SELECT 1 FROM agents WHERE id=task_turns.agent AND completion_pending IS NULL)",
    crate::runtime::features::executable!(
        "coalesce((SELECT comb_bound FROM agents WHERE id=task_turns.agent),0)"
    ),
    " ORDER BY agent LIMIT 128"
);
pub(super) const CLEAR_OUTCOME_DIRTY: &str = "UPDATE task_turns SET outcome_dirty=0 WHERE \
    agent=?1";
pub(super) const DISARMED_CUTOFFS: &str = concat!(
    "SELECT id FROM agents
WHERE json_extract(data,'$.auth_switch_pending.continuation') IS NOT NULL
AND (task_outcome!='open' OR expired=1 OR paused=1 OR control='stop' OR blocked=1
    OR json_extract(data,'$.recovery_fence') IS NOT NULL
    OR json_extract(data,'$.auth_switch_pending.source_generation') IS NOT json_extract(data,\
    '$.auth_generation')
    OR json_extract(data,'$.auth_switch_pending.source_execution') IS NOT execution_id
    OR NOT EXISTS(SELECT 1 FROM messages WHERE id=json_extract(agents.data,\
    '$.auth_switch_pending.continuation.message')
        AND recipient=agents.id AND state IN ('accepted','scheduled','submitting',\
    'submitted')))",
    crate::runtime::features::executable!("comb_bound"),
    " ORDER BY id LIMIT 128"
);
pub(super) const ADMISSION: &str = "SELECT blocked,claimed,pending_count>0,
EXISTS(SELECT 1 FROM messages WHERE recipient=?1 AND mode='next-turn' AND state='scheduled')
FROM agents WHERE id=?1";
pub(super) const WAITING: &str = "SELECT CASE
WHEN EXISTS(SELECT 1 FROM approvals WHERE agent=?1 AND state='pending') THEN 'question'
WHEN EXISTS(SELECT 1 FROM joins WHERE requester=?1 AND (state='collecting'
    OR state='deadline_passed' AND EXISTS(SELECT 1 FROM join_members WHERE join_id=joins.id \
    AND outcome='pending')))
    OR EXISTS(SELECT 1 FROM messages WHERE recipient=?1 AND kind='join_result' AND state IN \
    ('accepted','scheduled','submitting','submitted')) THEN 'join'
WHEN EXISTS(SELECT 1 FROM agents WHERE parent_id=?1 AND task_outcome='open' AND expired=0) \
    THEN 'children'
WHEN branch_hold>0 OR branch_gate IN ('waiting_for_source','ready_to_fork','forking') THEN \
    'branch'
WHEN EXISTS(SELECT 1 FROM messages WHERE recipient=?1 AND mode='next-turn' AND state IN \
    ('accepted','scheduled','submitting','submitted')) THEN 'input'
ELSE 'none' END FROM agents WHERE id=?1";

#[cfg(test)]
mod tests;
