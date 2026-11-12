//! One complete read-only setup plan, followed by the existing apply boundary.
use super::{
    CommandOutcome, Context, PreparedParts, Result, apply_setup, finish_setup, options, plan::Plan,
    prepare_parts, preview, publication::Snapshot, terminal::Terminal,
};
use std::path::PathBuf;

pub(super) struct PreparedSetup {
    pub(super) snapshot: Snapshot,
    pub(super) options: options::Values,
    plan: Plan,
    parts: PreparedParts,
    skill_dir: PathBuf,
}
impl PreparedSetup {
    pub(super) fn inspect(context: Context<'_>) -> Result<Self> {
        Self::inspect_resources(context, None)
    }
    pub(super) fn inspect_resources(
        context: Context<'_>,
        resources: Option<super::resources::View>,
    ) -> Result<Self> {
        let snapshot = Snapshot::inspect(context.config_path)?;
        let mut terminal = Terminal::open(context.options.yes == Some(true))?;
        let mut plan = Plan::inspect(&snapshot, context.options, &mut terminal)?;
        let options = plan.resolved_values(context.options.clone());
        let resources = resources.map_or_else(|| super::resources::View::normal(&options), Ok)?;
        let skill_dir = resources.skills_target.clone();
        let parts = prepare_parts(
            Context {
                options: &options,
                ..context
            },
            &snapshot,
            &mut plan,
            &resources,
        )?;
        Ok(Self {
            snapshot,
            options,
            plan,
            parts,
            skill_dir,
        })
    }
    pub(super) fn run(self, context: Context<'_>) -> Result<CommandOutcome> {
        let Self {
            snapshot,
            options,
            mut plan,
            parts,
            skill_dir,
        } = self;
        let context = Context {
            options: &options,
            ..context
        };
        if options.dry_run == Some(true) {
            return Ok(preview(context, &snapshot, &plan, parts));
        }
        let applied = apply_setup(
            context,
            &snapshot,
            &mut plan,
            parts.comb.as_ref(),
            &parts.copies,
        )?;
        finish_setup(context, plan, parts.comb.as_ref(), applied, &skill_dir)
    }
}
