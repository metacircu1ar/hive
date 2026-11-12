pub(super) const INHERIT_CLAUDE_BINDINGS: &str = "INSERT OR IGNORE INTO native_sessions \
    (agent,generation,provider,thread) SELECT ?1,generation,provider,thread FROM native_sessions \
    WHERE agent=?2 AND generation=?3 AND provider='claude'";

pub(super) const OWNED_CLAUDE_PATHS: &str = "SELECT path FROM native_session_paths \
    WHERE agent=?1 AND generation=?2 AND provider='claude' AND thread=?3 ORDER BY path LIMIT 129";

pub(super) const SCOPE_SESSIONS: &str = "SELECT min(n.agent), n.generation,n.provider,n.thread \
    FROM native_sessions n JOIN purge_scope p ON p.id=n.agent \
    WHERE (n.generation,n.provider,n.thread)>(?1,?2,?3) \
    GROUP BY n.generation,n.provider,n.thread ORDER BY n.generation,n.provider,n.thread LIMIT 128";

pub(super) const AGENT_SESSIONS: &str = "SELECT agent,generation,provider,thread FROM native_sessions \
    WHERE agent=?1 AND (generation,provider,thread)>(?2,?3,?4) \
    ORDER BY generation,provider,thread LIMIT 128";

pub(super) const ADMISSION_SURVIVING_OWNER: &str = "SELECT EXISTS(SELECT 1 FROM native_sessions n \
    WHERE n.generation=?1 AND n.provider=?2 AND n.thread=?3 AND n.agent!=?4 \
    AND NOT EXISTS(SELECT 1 FROM purge_scope p WHERE p.id=n.agent))";

pub(super) const INSERT_JOB: &str = "INSERT INTO native_cleanup_jobs(request,data) VALUES (?1,?2)";

pub(super) const INSERT_SESSION: &str = "INSERT INTO native_cleanup_sessions \
    (request,agent,generation,provider,thread,remaining) VALUES (?1,?2,?3,?4,?5,?6) RETURNING id";

pub(super) const PENDING_FILES: &str = "SELECT f.id,s.generation,s.provider,s.thread,f.data, \
    EXISTS(SELECT 1 FROM native_sessions n WHERE n.generation=s.generation AND n.provider=s.provider AND n.thread=s.thread) \
    FROM native_cleanup_files f JOIN native_cleanup_sessions s ON s.id=f.session ORDER BY f.id LIMIT 128";

pub(super) const FILE_SESSION: &str = "SELECT s.id,s.request,s.agent,s.remaining,s.deleted,s.absent,s.retained,s.bytes,s.warning \
    FROM native_cleanup_sessions s JOIN native_cleanup_files f ON f.session=s.id WHERE f.id=?1";

pub(super) const SAVE_SESSION: &str = "UPDATE native_cleanup_sessions SET remaining=?2,deleted=?3,absent=?4,retained=?5,bytes=?6,warning=?7 WHERE id=?1";
