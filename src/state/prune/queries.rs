//! Prune SQL statements and query builders.

pub(super) const NO_AGENT_REFERENCES: &str = r"
SELECT json_extract(data, '$.auth_generation'), 'agent'
FROM agents
UNION ALL
SELECT generation, 'cleanup' FROM native_cleanup_sessions
";
