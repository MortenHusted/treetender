//! Per-lane PostgreSQL databases managed through `psql`. Ownership is a marker
//! stored as the database comment; a matching name alone is never proof.
use crate::manifest::Adapter;
use crate::model::*;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Marker {
    treetender: u32,
    workspace_id: String,
    resource_id: String,
    owner_token: String,
    instance: String,
}

pub(crate) struct Server<'a> {
    command: &'a [String],
    conninfo: String,
    timeout: u64,
    pub database: &'a str,
}

impl<'a> Server<'a> {
    pub(crate) fn of(adapter: &'a Adapter) -> Option<Self> {
        let Adapter::Postgres {
            database,
            service,
            host,
            port,
            user,
            command,
            timeout_secs,
        } = adapter
        else {
            return None;
        };
        let mut conninfo = vec!["dbname=postgres".to_owned()];
        for (key, value) in [
            ("service", service.clone()),
            ("host", host.clone()),
            ("port", port.map(|p| p.to_string())),
            ("user", user.clone()),
        ] {
            if let Some(value) = value {
                let quoted = value.replace('\\', "\\\\").replace('\'', "\\'");
                conninfo.push(format!("{key}='{quoted}'"));
            }
        }
        Some(Self {
            command,
            conninfo: conninfo.join(" "),
            timeout: *timeout_secs,
            database,
        })
    }

    /// Runs a script with psql variables; values never enter SQL text directly.
    fn psql(
        &self,
        w: &Workspace,
        cwd: &Path,
        variables: &[(&str, &str)],
        script: &str,
    ) -> Result<String> {
        let mut argv = self.command.to_vec();
        argv.extend(
            [
                "-X",
                "-q",
                "-A",
                "-t",
                "-F",
                "\u{1f}",
                "-v",
                "ON_ERROR_STOP=1",
            ]
            .map(String::from),
        );
        for (name, value) in variables {
            argv.push("-v".into());
            argv.push(format!("{name}={value}"));
        }
        argv.push("-d".into());
        argv.push(self.conninfo.clone());
        let output = crate::adapter::run(
            &argv,
            cwd,
            &w.environment(),
            Some(script.as_bytes().to_vec()),
            self.timeout,
        )?;
        String::from_utf8(output)
            .map_err(|_| Error::new("adapter_output", "psql output is not UTF-8"))
    }
}

/// Names are quoted identifiers, but stay simple so they read the same in every tool.
pub(crate) fn valid_database(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 63
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        || ["postgres", "template0", "template1"].contains(&name)
    {
        return Err(Error::new(
            "manifest",
            format!(
                "database {name} must be 1-63 lowercase letters, digits or _, and not a system database"
            ),
        ));
    }
    Ok(())
}

pub(crate) fn inspect(server: &Server, w: &Workspace, cwd: &Path) -> Result<Proof> {
    Ok(observe(server, w, cwd)?.0)
}

/// The proof and the exact comment it was read from.
fn observe(server: &Server, w: &Workspace, cwd: &Path) -> Result<(Proof, String)> {
    valid_database(server.database)?;
    let output = server.psql(
        w,
        cwd,
        &[("name", server.database)],
        "SELECT oid, coalesce(shobj_description(oid, 'pg_database'), '') FROM pg_database WHERE datname = :'name';\n",
    )?;
    let output = output.trim_end_matches('\n');
    if output.is_empty() {
        return Ok((Proof::missing(), String::new()));
    }
    let (oid, comment) = output
        .split_once('\u{1f}')
        .ok_or_else(|| Error::new("adapter_output", "unexpected psql output while inspecting"))?;
    let oid: u32 = oid
        .parse()
        .map_err(|_| Error::new("adapter_output", "unexpected database oid"))?;
    let marker = serde_json::from_str::<Marker>(comment)
        .ok()
        .filter(|m| m.treetender == 1);
    Ok((
        Proof {
            presence: Presence::Present,
            handle: serde_json::json!({"database": server.database, "oid": oid}),
            instance: marker.as_ref().map(|m| m.instance.clone()),
            owner_token: marker.map(|m| m.owner_token),
        },
        comment.to_owned(),
    ))
}

fn marker(w: &Workspace, resource_id: &str, token: &str) -> Result<String> {
    Ok(serde_json::to_string(&Marker {
        treetender: 1,
        workspace_id: w.id.clone(),
        resource_id: resource_id.into(),
        owner_token: token.into(),
        instance: uuid::Uuid::new_v4().to_string(),
    })?)
}

pub(crate) fn prepare(
    server: &Server,
    w: &Workspace,
    cwd: &Path,
    resource_id: &str,
    token: &str,
) -> Result<Proof> {
    valid_database(server.database)?;
    server.psql(
        w,
        cwd,
        &[
            ("name", server.database),
            ("marker", &marker(w, resource_id, token)?),
        ],
        "CREATE DATABASE :\"name\";\nCOMMENT ON DATABASE :\"name\" IS :'marker';\n",
    )?;
    inspect(server, w, cwd)
}

/// Adoption needs lane-specific naming evidence: the workspace's ident.
pub(crate) fn adoptable(w: &Workspace, database: &str) -> Result<()> {
    if w.name.is_empty() || !database.contains(&w.ident()) {
        return Err(Error::new(
            "ownership",
            format!(
                "{database} does not contain this workspace's name ({}); only lane-specific databases can be adopted",
                w.ident()
            ),
        ));
    }
    Ok(())
}

pub(crate) fn adopt(
    server: &Server,
    w: &Workspace,
    cwd: &Path,
    resource_id: &str,
    token: &str,
    observed: Option<&Proof>,
) -> Result<Proof> {
    adoptable(w, server.database)?;
    let oid = observed
        .filter(|p| p.presence == Presence::Present && p.instance.is_none())
        .and_then(|p| p.handle.get("oid"))
        .and_then(|oid| oid.as_u64())
        .ok_or_else(|| Error::new("ownership", "adopt needs a reviewed unmarked database"))?
        .to_string();
    let output = server.psql(
        w,
        cwd,
        &[
            ("name", server.database),
            ("oid", &oid),
            ("marker", &marker(w, resource_id, token)?),
        ],
        "SELECT CASE WHEN (SELECT oid = :oid AND shobj_description(oid, 'pg_database') IS NULL FROM pg_database WHERE datname = :'name') THEN 'true' ELSE 'false' END AS unmarked \\gset\n\\if :unmarked\nCOMMENT ON DATABASE :\"name\" IS :'marker';\n\\else\n\\echo changed\n\\endif\n",
    )?;
    if output.contains("changed") {
        return Err(Error::new(
            "ownership",
            format!(
                "{} changed since the adopt plan was reviewed",
                server.database
            ),
        ));
    }
    inspect(server, w, cwd)
}

/// Drops the database only if its OID and marker still match the receipt,
/// rechecked in the same psql session immediately before the drop.
pub(crate) fn destroy(
    server: &Server,
    w: &Workspace,
    cwd: &Path,
    token: &str,
    receipt: Option<&Proof>,
) -> Result<Proof> {
    let (current, comment) = observe(server, w, cwd)?;
    if current.presence == Presence::Missing {
        return Ok(current);
    }
    if Some(&current) != receipt || current.owner_token.as_deref() != Some(token) {
        return Err(Error::new(
            "ownership",
            format!(
                "{} differs from its receipt; nothing dropped",
                server.database
            ),
        ));
    }
    let oid = current.handle["oid"].to_string();
    let output = server.psql(
        w,
        cwd,
        &[("name", server.database), ("oid", &oid), ("marker", &comment)],
        "SELECT CASE WHEN (SELECT oid = :oid AND shobj_description(oid, 'pg_database') = :'marker' FROM pg_database WHERE datname = :'name') THEN 'true' ELSE 'false' END AS verified \\gset\n\\if :verified\nDROP DATABASE :\"name\" WITH (FORCE);\n\\else\n\\echo changed\n\\endif\n",
    )?;
    if output.contains("changed") {
        return Err(Error::new(
            "ownership",
            format!(
                "{} changed before the drop; nothing dropped",
                server.database
            ),
        ));
    }
    inspect(server, w, cwd)
}
