use anyhow::Result;
use rusqlite::Connection;

use crate::format;
use crate::hierarchy;
use crate::models::project;

pub fn run(conn: &Connection, project_name: Option<&str>, auto: bool) -> Result<()> {
    let proj = if auto {
        let cwd = std::env::current_dir()?;
        project::detect_from_cwd(conn, cwd.to_str().unwrap_or(""))?
    } else {
        project_name.map(String::from)
    };

    let assembly = hierarchy::assemble(conn, proj.as_deref())?;
    print!("{}", format::context_to_markdown(&assembly));

    Ok(())
}
