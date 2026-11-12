pub(super) const MEMBERS: &str = "SELECT position, message_id, target, outcome, output, \
    output_final, task_completed, delivered, task_scoped, task_selection FROM join_members WHERE join_id=?1 ORDER BY \
    position";

pub(super) const TASK_OWNERS: &str = "SELECT m.join_id,m.requester FROM join_members m JOIN joins j ON j.id=m.join_id
WHERE m.target=?1 AND m.task_scoped=1 AND m.outcome='pending' AND j.state IN ('collecting','deadline_passed')";
