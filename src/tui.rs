use anyhow::{Result, bail};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use std::{
    collections::BTreeSet,
    io::{self, IsTerminal},
    time::Duration,
};
use treetender::{Engine, Operation, Plan, Selectors, Workspace};

struct ResourceRow {
    resource_id: String,
    state: String,
}

struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, crossterm::cursor::Show);
    }
}

struct App {
    workspace: String,
    workspaces: Vec<Workspace>,
    resources: Vec<ResourceRow>,
    cursor: usize,
    retained: BTreeSet<String>,
    plan: Option<Plan>,
    detail: String,
    message: String,
    scroll: u16,
}

impl App {
    fn new(workspace: String) -> Self {
        Self {
            workspace,
            workspaces: vec![],
            resources: vec![],
            cursor: 0,
            retained: BTreeSet::new(),
            plan: None,
            detail: String::new(),
            message: "Read-only until you register or apply a reviewed plan.".into(),
            scroll: 0,
        }
    }

    fn refresh(&mut self, engine: &Engine) -> Result<()> {
        self.workspaces = engine.list()?;
        match engine.status(&self.workspace) {
            Ok(status) => {
                self.workspace = status.workspace.id.clone();
                self.resources = status
                    .resources
                    .iter()
                    .map(|r| ResourceRow {
                        resource_id: r.resource_id.clone(),
                        state: r.state.clone(),
                    })
                    .collect();
                for resource in &status.declarations.resources {
                    if let Some(row) = self
                        .resources
                        .iter_mut()
                        .find(|r| r.resource_id == resource.id)
                    {
                        if resource.is_passive() {
                            row.state = format!("passive; retained {} receipt", row.state);
                        }
                    } else {
                        self.resources.push(ResourceRow {
                            resource_id: resource.id.clone(),
                            state: if resource.is_passive() {
                                "passive · declared only".into()
                            } else {
                                format!("{:?} declaration", resource.ownership)
                            },
                        });
                    }
                }
                self.detail = serde_json::to_string_pretty(&status)?;
            }
            Err(error) => {
                self.resources.clear();
                self.detail = error.to_string();
            }
        }
        self.cursor = self.cursor.min(self.resources.len().saturating_sub(1));
        self.scroll = 0;
        Ok(())
    }

    fn toggle_retain(&mut self) {
        if let Some(resource) = self.resources.get(self.cursor) {
            if !self.retained.remove(&resource.resource_id) {
                self.retained.insert(resource.resource_id.clone());
            }
            self.plan = None;
            self.message =
                "Retention selection changed; generate a new plan before applying.".into();
        }
    }

    fn select_workspace(&mut self, direction: isize, engine: &Engine) -> Result<()> {
        if self.workspaces.is_empty() {
            return Ok(());
        }
        let current = self
            .workspaces
            .iter()
            .position(|w| w.id == self.workspace || w.path.to_string_lossy() == self.workspace)
            .unwrap_or(0);
        let index =
            (current as isize + direction).rem_euclid(self.workspaces.len() as isize) as usize;
        self.workspace = self.workspaces[index].id.clone();
        self.plan = None;
        self.retained.clear();
        self.cursor = 0;
        self.refresh(engine)
    }

    fn make_plan(&mut self, operation: Operation, engine: &Engine) -> Result<()> {
        let only = if operation == Operation::Forget {
            self.resources
                .get(self.cursor)
                .map(|r| vec![r.resource_id.clone()])
                .unwrap_or_default()
        } else {
            vec![]
        };
        let plan = engine.plan(
            &self.workspace,
            operation,
            Selectors {
                only,
                retain: self.retained.iter().cloned().collect(),
            },
        )?;
        self.detail = serde_json::to_string_pretty(&plan)?;
        self.message = if plan.blockers.is_empty() {
            format!(
                "Review {:?}: {} steps, {} exclusions. Press a to apply this exact plan.",
                plan.operation,
                plan.steps.len(),
                plan.exclusions.len()
            )
        } else {
            format!(
                "Blocked: {}. No operation will be applied.",
                plan.blockers.join("; ")
            )
        };
        self.scroll = 0;
        self.plan = Some(plan);
        Ok(())
    }

    fn apply(&mut self, engine: &Engine) -> Result<()> {
        let Some(plan) = self.plan.take() else {
            self.message = "Choose an operation and review its plan before applying.".into();
            return Ok(());
        };
        if !plan.blockers.is_empty() {
            self.message = "The plan has blockers. Resolve them and generate a new plan.".into();
            self.plan = Some(plan);
            return Ok(());
        }
        let result = engine.apply(&plan)?;
        self.refresh(engine)?;
        self.detail = serde_json::to_string_pretty(&result)?;
        self.message = if result.success {
            "Applied. Completed receipts are recorded.".into()
        } else {
            "Operation failed; completed receipts are retained. Review recovery details.".into()
        };
        Ok(())
    }
}

pub fn run(engine: Engine, workspace: String) -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("TUI requires an interactive terminal; use --json status/plan/apply for automation");
    }
    let mut app = App::new(workspace);
    app.refresh(&engine)?;
    enable_raw_mode()?;
    let _guard = TerminalGuard;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    terminal.clear()?;
    loop {
        terminal.draw(|frame| draw(frame, &app))?;
        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        let operation = match key.code {
            KeyCode::Char('1') => Some(Operation::Prepare),
            KeyCode::Char('2') => Some(Operation::Build),
            KeyCode::Char('3') => Some(Operation::Check),
            KeyCode::Char('4') => Some(Operation::Start),
            KeyCode::Char('5') => Some(Operation::Stop),
            KeyCode::Char('6') => Some(Operation::Clean),
            KeyCode::Char('7') => Some(Operation::Forget),
            _ => None,
        };
        let result = if let Some(operation) = operation {
            app.make_plan(operation, &engine)
        } else {
            match key.code {
                KeyCode::Char('q') => break,
                KeyCode::Char('a') => app.apply(&engine),
                KeyCode::Char('r') => engine
                    .register(
                        &app.workspaces
                            .iter()
                            .find(|w| w.id == app.workspace)
                            .map(|w| w.path.clone())
                            .unwrap_or_else(|| std::path::PathBuf::from(&app.workspace)),
                    )
                    .map_err(anyhow::Error::from)
                    .and_then(|_| app.refresh(&engine)),
                KeyCode::Char('g') => {
                    app.plan = None;
                    app.refresh(&engine)
                }
                KeyCode::Char('[') => app.select_workspace(-1, &engine),
                KeyCode::Char(']') => app.select_workspace(1, &engine),
                KeyCode::Down | KeyCode::Char('j') => {
                    app.cursor = (app.cursor + 1).min(app.resources.len().saturating_sub(1));
                    Ok(())
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    app.cursor = app.cursor.saturating_sub(1);
                    Ok(())
                }
                KeyCode::Char(' ') => {
                    app.toggle_retain();
                    Ok(())
                }
                KeyCode::PageDown => {
                    app.scroll = app.scroll.saturating_add(10);
                    Ok(())
                }
                KeyCode::PageUp => {
                    app.scroll = app.scroll.saturating_sub(10);
                    Ok(())
                }
                KeyCode::Esc => {
                    app.plan = None;
                    app.refresh(&engine)
                }
                _ => Ok(()),
            }
        };
        if let Err(error) = result {
            app.message = format!("{error:#}");
        }
    }
    Ok(())
}

fn draw(frame: &mut Frame<'_>, app: &App) {
    let vertical = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(5),
        Constraint::Length(6),
    ])
    .split(frame.area());
    let selected = app
        .workspaces
        .iter()
        .find(|w| w.id == app.workspace || w.path.to_string_lossy() == app.workspace);
    let primary = selected.is_some_and(|w| w.primary);
    let heading = format!(
        "{}{} · {} registered",
        selected
            .map(|w| w.path.to_string_lossy().into_owned())
            .unwrap_or_else(|| app.workspace.clone()),
        if primary { " [PRIMARY: protected]" } else { "" },
        app.workspaces.len()
    );
    frame.render_widget(
        Paragraph::new(heading).block(
            Block::default()
                .title("TreeTender · [ / ] switch workspace")
                .borders(Borders::ALL),
        ),
        vertical[0],
    );
    let middle = Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(vertical[1]);
    let items: Vec<ListItem<'_>> = app
        .resources
        .iter()
        .map(|r| {
            ListItem::new(format!(
                "{} {} · {}",
                if app.retained.contains(&r.resource_id) {
                    "keep"
                } else {
                    "    "
                },
                r.resource_id,
                r.state
            ))
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::default()
                .title("Resources · space retains")
                .borders(Borders::ALL),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    let mut state =
        ListState::default().with_selected((!app.resources.is_empty()).then_some(app.cursor));
    frame.render_stateful_widget(list, middle[0], &mut state);
    frame.render_widget(
        Paragraph::new(app.detail.as_str())
            .block(
                Block::default()
                    .title(if app.plan.is_some() {
                        "Review plan · PgUp/PgDn"
                    } else {
                        "State / result · PgUp/PgDn"
                    })
                    .borders(Borders::ALL),
            )
            .scroll((app.scroll, 0)),
        middle[1],
    );
    let footer = vec![
        Line::from("1 prepare  2 build  3 check  4 start  5 stop  6 clean  7 forget selected"),
        Line::from("a apply reviewed plan · r register · g refresh · Esc cancel · q quit"),
        Line::from(app.message.as_str()).style(Style::default().fg(Color::Yellow)),
    ];
    frame.render_widget(
        Paragraph::new(footer)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL)),
        vertical[2],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    #[test]
    fn passive_inventory_renders_without_allocations_or_observations() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let initialized = std::process::Command::new("git")
            .args(["init", "--quiet"])
            .arg(&repo)
            .status()
            .unwrap();
        assert!(initialized.success());
        std::fs::write(
            repo.join("treetender.toml"),
            include_str!("../examples/inventory.toml"),
        )
        .unwrap();
        let engine = Engine::open(temp.path().join("state")).unwrap();
        let workspace = engine.register(&repo).unwrap();
        let mut app = App::new(workspace.id);
        app.refresh(&engine).unwrap();
        assert_eq!(app.resources.len(), 2);
        assert!(
            app.resources
                .iter()
                .all(|row| row.state.contains("passive"))
        );
        let mut terminal = Terminal::new(TestBackend::new(130, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("database-server"));
        assert!(text.contains("passive"));
        let status = engine.status(&app.workspace).unwrap();
        assert!(status.observations.is_empty());
        assert!(status.resources.is_empty());
    }

    #[test]
    fn blocked_plan_is_visible_and_can_be_cancelled_without_effects() {
        let mut app = App::new("fixture".into());
        app.message = "Blocked: primary resources are protected".into();
        app.detail = "shared development database: excluded".into();
        let mut terminal = Terminal::new(TestBackend::new(100, 25)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer();
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("primary resources are protected"));
        assert!(text.contains("shared development database: excluded"));
        assert!(text.contains("a apply reviewed plan"));
        assert!(app.plan.is_none());
    }
}
