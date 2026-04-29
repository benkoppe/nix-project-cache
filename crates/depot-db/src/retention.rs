use anyhow::{Context as _, Result, bail};
use uuid::Uuid;
use wildmatch::WildMatch;

use depot_core::project::ProjectSlug;

use crate::models::{
    ProjectRefRetentionRow, ProjectRetentionPolicyLookupRow, ProjectRetentionPolicyRecord,
    ProjectRetentionRuleLookupRow, ProjectRetentionRuleRecord, RetainedBuildLookupRow,
};
use crate::pool::SqliteDatabase;

const DEFAULT_KEEP_LATEST_BUILDS_PER_REF: u32 = 2;
const DEFAULT_OBJECT_DELETE_GRACE_SECONDS: u64 = 24 * 60 * 60;

pub fn default_retention_rules() -> Vec<ProjectRetentionRuleRecord> {
    vec![
        ProjectRetentionRuleRecord {
            priority: 10,
            ref_pattern: "refs/heads/main".to_owned(),
            ttl_seconds: None,
            keep_builds: Some(2),
        },
        ProjectRetentionRuleRecord {
            priority: 20,
            ref_pattern: "refs/heads/master".to_owned(),
            ttl_seconds: None,
            keep_builds: Some(2),
        },
        ProjectRetentionRuleRecord {
            priority: 30,
            ref_pattern: "refs/heads/trunk".to_owned(),
            ttl_seconds: None,
            keep_builds: Some(2),
        },
        ProjectRetentionRuleRecord {
            priority: 40,
            ref_pattern: "refs/tags/*".to_owned(),
            ttl_seconds: None,
            keep_builds: Some(1),
        },
        ProjectRetentionRuleRecord {
            priority: 50,
            ref_pattern: "refs/heads/release/*".to_owned(),
            ttl_seconds: Some(180 * 24 * 60 * 60),
            keep_builds: Some(2),
        },
        ProjectRetentionRuleRecord {
            priority: 60,
            ref_pattern: "refs/pull/*".to_owned(),
            ttl_seconds: Some(14 * 24 * 60 * 60),
            keep_builds: Some(1),
        },
        ProjectRetentionRuleRecord {
            priority: 70,
            ref_pattern: "refs/merge-requests/*".to_owned(),
            ttl_seconds: Some(14 * 24 * 60 * 60),
            keep_builds: Some(1),
        },
        ProjectRetentionRuleRecord {
            priority: 80,
            ref_pattern: "refs/heads/*".to_owned(),
            ttl_seconds: Some(60 * 24 * 60 * 60),
            keep_builds: Some(2),
        },
        ProjectRetentionRuleRecord {
            priority: 90,
            ref_pattern: "*".to_owned(),
            ttl_seconds: Some(30 * 24 * 60 * 60),
            keep_builds: Some(1),
        },
    ]
}

impl SqliteDatabase {
    pub async fn get_project_retention_policy(
        &self,
        project: &ProjectSlug,
    ) -> Result<ProjectRetentionPolicyRecord> {
        let project_id = self.project_id_by_slug(project).await?;

        let custom_policy = sqlx::query_as!(
            ProjectRetentionPolicyLookupRow,
            r#"
            SELECT
                p.slug AS project_slug,
                0 AS "inherited_default!: i64",
                rp.keep_latest_builds_per_ref,
                rp.object_delete_grace_seconds
            FROM project_retention_policies rp
            JOIN projects p ON p.id = rp.project_id
            WHERE rp.project_id = ?
            "#,
            project_id
        )
        .fetch_optional(&self.pool)
        .await
        .context("loading project retention policy")?;

        if let Some(policy) = custom_policy {
            let rules = sqlx::query_as!(
                ProjectRetentionRuleLookupRow,
                r#"
                SELECT
                    priority,
                    ref_pattern,
                    ttl_seconds,
                    keep_builds
                FROM project_ref_retention_rules
                WHERE project_id = ?
                ORDER BY priority ASC
                "#,
                project_id,
            )
            .fetch_all(&self.pool)
            .await
            .context("loading project retention rules")?
            .into_iter()
            .map(ProjectRetentionRuleLookupRow::into_record)
            .collect::<Result<Vec<_>>>()?;

            return policy.into_record(rules);
        }

        Ok(ProjectRetentionPolicyRecord {
            project_slug: project.clone(),
            inherited_default: true,
            keep_latest_builds_per_ref: DEFAULT_KEEP_LATEST_BUILDS_PER_REF,
            object_delete_grace_seconds: DEFAULT_OBJECT_DELETE_GRACE_SECONDS,
            rules: default_retention_rules(),
        })
    }

    pub async fn replace_project_retention_policy(
        &self,
        project: &ProjectSlug,
        keep_latest_builds_per_ref: u32,
        object_delete_grace_seconds: u64,
        rules: &[ProjectRetentionRuleRecord],
    ) -> Result<()> {
        if keep_latest_builds_per_ref == 0 {
            bail!("keep_latest_builds_per_ref must be greater than zero");
        }

        if rules.is_empty() {
            bail!("retention policy must contain at least one rule");
        }

        let project_id = self.project_id_by_slug(project).await?;
        let keep_latest_builds_per_ref = i64::from(keep_latest_builds_per_ref);
        let object_delete_grace_seconds =
            i64::try_from(object_delete_grace_seconds).context("converting grace seconds")?;

        let mut tx = self
            .pool()
            .begin()
            .await
            .context("beginning replace_project_retention_policy transaction")?;

        sqlx::query!(
            r#"
            INSERT INTO project_retention_policies (
                project_id,
                keep_latest_builds_per_ref,
                object_delete_grace_seconds
            )
            VALUES (?, ?, ?)
            ON CONFLICT(project_id) DO UPDATE SET
                keep_latest_builds_per_ref = excluded.keep_latest_builds_per_ref,
                object_delete_grace_seconds = excluded.object_delete_grace_seconds,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
            "#,
            project_id,
            keep_latest_builds_per_ref,
            object_delete_grace_seconds,
        )
        .execute(&mut *tx)
        .await
        .context("upserting project retention policy")?;

        sqlx::query!(
            r#"
            DELETE FROM project_ref_retention_rules
            WHERE project_id = ?
            "#,
            project_id,
        )
        .execute(&mut *tx)
        .await
        .context("deleting existing project retention rules")?;

        for rule in rules {
            if rule.priority == 0 {
                bail!("retention rule priority must be greater than zero");
            }

            if rule.ref_pattern.trim().is_empty() {
                bail!("retention rule ref_pattern must not be empty");
            }

            let rule_id = Uuid::now_v7().to_string();
            let priority = i64::from(rule.priority);
            let ref_pattern = rule.ref_pattern.trim();
            let ttl_seconds = rule
                .ttl_seconds
                .map(i64::try_from)
                .transpose()
                .context("converting ttl_seconds")?;
            let keep_builds = rule.keep_builds.map(i64::from);

            sqlx::query!(
                r#"
                INSERT INTO project_ref_retention_rules (
                    id,
                    project_id,
                    priority,
                    ref_pattern,
                    ttl_seconds,
                    keep_builds
                )
                VALUES (?, ?, ?, ?, ?, ?)
                "#,
                rule_id,
                project_id,
                priority,
                ref_pattern,
                ttl_seconds,
                keep_builds,
            )
            .execute(&mut *tx)
            .await
            .context("inserting project retention rule")?;
        }

        tx.commit()
            .await
            .context("committing replace_project_retention_policy transaction")?;

        Ok(())
    }

    pub async fn delete_project_retention_policy(&self, project: &ProjectSlug) -> Result<bool> {
        let project_id = self.project_id_by_slug(project).await?;

        let result = sqlx::query!(
            r#"
            DELETE FROM project_retention_policies
            WHERE project_id = ?
            "#,
            project_id,
        )
        .execute(&self.pool)
        .await
        .context("deleting project retention policy")?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn list_retained_build_ids_for_gc(&self) -> Result<Vec<String>> {
        let refs = sqlx::query_as!(
            ProjectRefRetentionRow,
            r#"
            SELECT
                p.slug AS project_slug,
                pr.ref_name,
                pr.updated_at
            FROM project_refs pr
            JOIN projects p ON p.id = pr.project_id
            ORDER BY p.slug ASC, pr.ref_name ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .context("listing project refs for retention")?;

        let mut retained = Vec::new();

        for project_ref in refs {
            let project = ProjectSlug::parse(&project_ref.project_slug).map_err(|_| {
                anyhow::anyhow!("invalid project slug {}", project_ref.project_slug)
            })?;
            let policy = self.get_project_retention_policy(&project).await?;

            let Some(rule) = matching_retention_rule(&policy.rules, &project_ref.ref_name) else {
                continue;
            };

            if !ref_is_active(&project_ref.updated_at, rule.ttl_seconds)? {
                continue;
            }

            let keep_builds = rule
                .keep_builds
                .unwrap_or(policy.keep_latest_builds_per_ref);

            let project_id = self.project_id_by_slug(&project).await?;
            let limit = i64::from(keep_builds);

            let rows = sqlx::query_as!(
                RetainedBuildLookupRow,
                r#"
                SELECT id AS build_id
                FROM builds
                WHERE project_id = ?
                  AND ref_name = ?
                  AND status = 'finalized'
                  AND finalized_at IS NOT NULL
                ORDER BY finalized_at DESC, created_at DESC
                LIMIT ?
                "#,
                project_id,
                project_ref.ref_name,
                limit,
            )
            .fetch_all(&self.pool)
            .await
            .context("listing retained builds for ref")?;

            retained.extend(rows.into_iter().map(|row| row.build_id));
        }

        retained.sort();
        retained.dedup();

        Ok(retained)
    }
}

fn matching_retention_rule<'a>(
    rules: &'a [ProjectRetentionRuleRecord],
    ref_name: &str,
) -> Option<&'a ProjectRetentionRuleRecord> {
    rules
        .iter()
        .filter(|rule| WildMatch::new(&rule.ref_pattern).matches(ref_name))
        .min_by_key(|rule| rule.priority)
}

fn ref_is_active(updated_at: &str, ttl_seconds: Option<u64>) -> Result<bool> {
    let Some(ttl_seconds) = ttl_seconds else {
        return Ok(true);
    };

    let updated_at =
        time::OffsetDateTime::parse(updated_at, &time::format_description::well_known::Rfc3339)
            .context("parsing project ref updated_at")?;

    let age = time::OffsetDateTime::now_utc() - updated_at;
    Ok(age.whole_seconds() < i64::try_from(ttl_seconds).context("converting ttl_seconds")?)
}
