mod cli;
mod command;
mod diff;
mod git;
mod items;
mod process;
mod screen;
mod status;
mod theme;
mod ui;

use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    terminal::{
        self, disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
    },
    ExecutableCommand,
};
use diff::Hunk;
use items::{Act, Item};
use ratatui::prelude::CrosstermBackend;
use screen::Screen;
use std::{
    io::{self, stderr, Stderr},
    process::{Command, Stdio},
};

type Terminal = ratatui::Terminal<CrosstermBackend<Stderr>>;

lazy_static::lazy_static! {
    static ref USE_DELTA: bool = Command::new("delta").output().map(|out| out.status.success()).unwrap_or(false);
}

struct State {
    quit: bool,
    screens: Vec<Screen>,
    terminal: Terminal,
}

fn main() -> io::Result<()> {
    let mut state = create_initial_state(cli::Cli::parse())?;

    state.terminal.hide_cursor()?;

    enable_raw_mode()?;
    stderr().execute(EnterAlternateScreen)?;

    run_app(&mut state)?;

    stderr().execute(LeaveAlternateScreen)?;
    disable_raw_mode()?;
    Ok(())
}

fn create_initial_state(args: cli::Cli) -> io::Result<State> {
    let size = terminal::size()?;
    let screens = match args.command {
        Some(cli::Commands::Show { git_show_args }) => {
            vec![screen::show::create(size, git_show_args)]
        }
        Some(cli::Commands::Log { git_log_args }) => vec![screen::log::create(size, git_log_args)],
        None => vec![screen::status::create(size)],
    };

    Ok(State {
        quit: false,
        screens,
        terminal: Terminal::new(CrosstermBackend::new(stderr()))?,
    })
}

fn run_app(state: &mut State) -> Result<(), io::Error> {
    while !state.quit {
        if let Some(screen) = state.screens.last_mut() {
            state.terminal.draw(|frame| ui::ui(frame, screen))?;
            screen.handle_command_output();
        }

        handle_events(state)?;

        if let Some(screen) = state.screens.last_mut() {
            screen.clamp_cursor();
        }
    }

    Ok(())
}

fn handle_events(state: &mut State) -> io::Result<()> {
    // TODO Won't need to poll all the time if command outputs were handled async
    if !event::poll(std::time::Duration::from_millis(100))? {
        return Ok(());
    }

    let Some(screen) = state.screens.last_mut() else {
        panic!("No screen");
    };

    match event::read()? {
        Event::Resize(w, h) => screen.size = (w, h),
        Event::Key(key) => {
            if key.kind == KeyEventKind::Press {
                screen.clear_finished_command();

                match (key.modifiers, key.code) {
                    // Generic
                    (KeyModifiers::NONE, KeyCode::Char('q')) => state.quit = true,
                    (KeyModifiers::NONE, KeyCode::Char('g')) => screen.refresh_items(),

                    // Navigation
                    (KeyModifiers::NONE, KeyCode::Tab) => screen.toggle_section(),
                    (KeyModifiers::NONE, KeyCode::Char('k')) => screen.select_previous(),
                    (KeyModifiers::NONE, KeyCode::Char('j')) => screen.select_next(),

                    (KeyModifiers::CONTROL, KeyCode::Char('u')) => screen.scroll_half_page_up(),
                    (KeyModifiers::CONTROL, KeyCode::Char('d')) => screen.scroll_half_page_down(),

                    // Listing / showing
                    (KeyModifiers::NONE, KeyCode::Char('l')) => {
                        goto_log_screen(&mut state.screens)?
                    }

                    // Commands
                    (KeyModifiers::NONE, KeyCode::Char('f')) => {
                        screen.issue_command(&[], git::fetch_all_cmd())?;
                        screen.refresh_items();
                    }

                    // Actionables
                    (KeyModifiers::NONE, KeyCode::Enter) => {
                        if let Some(act) = &screen.get_selected_item().act {
                            match act {
                                Act::Ref(r) => goto_show_screen(r.clone(), &mut state.screens)?,
                                Act::Untracked(f) => {
                                    open_subscreen(&mut state.terminal, &[], editor_cmd(f, None))?;
                                    screen.refresh_items();
                                }
                                Act::Delta(d) => {
                                    let terminal: &mut Terminal = &mut state.terminal;
                                    open_subscreen(terminal, &[], editor_cmd(&d.new_file, None))?;
                                    screen.refresh_items();
                                }
                                Act::Hunk(h) => {
                                    open_subscreen(
                                        &mut state.terminal,
                                        &[],
                                        editor_cmd(&h.new_file, Some(&h)),
                                    )?;
                                    screen.refresh_items();
                                }
                                Act::DiffLine(_) => {
                                    todo!()
                                }
                            }
                        }
                    }

                    (KeyModifiers::NONE, KeyCode::Char('s')) => {
                        if let Some(act) = &screen.get_selected_item().act {
                            match act {
                                Act::Ref(_) => {
                                    todo!()
                                }
                                Act::Untracked(f) => {
                                    screen.issue_command(&[], git::stage_file_cmd(f))?;
                                }
                                Act::Delta(d) => {
                                    screen.issue_command(&[], git::stage_file_cmd(&d.new_file))?
                                }
                                Act::Hunk(h) => {
                                    screen.issue_command(
                                        h.format_patch().as_bytes(),
                                        git::stage_patch_cmd(),
                                    )?;
                                }
                                Act::DiffLine(_) => {
                                    todo!()
                                }
                            }
                        }

                        screen.refresh_items();
                    }
                    (KeyModifiers::NONE, KeyCode::Char('u')) => {
                        if let Some(act) = &screen.get_selected_item().act {
                            match act {
                                Act::Ref(_) => {
                                    todo!()
                                }
                                Act::Untracked(_) => {
                                    todo!()
                                }
                                Act::Delta(d) => {
                                    screen.issue_command(&[], git::unstage_file_cmd(d))?
                                }
                                Act::Hunk(h) => {
                                    screen.issue_command(
                                        h.format_patch().as_bytes(),
                                        git::unstage_patch_cmd(),
                                    )?;
                                }
                                Act::DiffLine(_) => {
                                    todo!()
                                }
                            }
                        }

                        screen.refresh_items();
                    }
                    (KeyModifiers::NONE, KeyCode::Char('c')) => {
                        open_subscreen(&mut state.terminal, &[], git::commit_cmd())?;
                        screen.refresh_items();
                    }
                    (KeyModifiers::SHIFT, KeyCode::Char('P')) => {
                        screen.issue_command(&[], git::push_cmd())?
                    }
                    (KeyModifiers::NONE, KeyCode::Char('p')) => {
                        screen.issue_command(&[], git::pull_cmd())?
                    }
                    _ => (),
                }
            }
        }
        _ => (),
    }

    if state.quit {
        state.screens.pop();
        if let Some(screen) = state.screens.last_mut() {
            state.quit = false;
            screen.refresh_items();
        }
    }

    Ok(())
}

fn goto_show_screen(reference: String, screens: &mut Vec<Screen>) -> Result<(), io::Error> {
    screens.push(screen::show::create(terminal::size()?, vec![reference]));
    Ok(())
}

fn goto_log_screen(screens: &mut Vec<Screen>) -> Result<(), io::Error> {
    screens.drain(1..);
    screens.push(screen::log::create(terminal::size()?, vec![]));
    Ok(())
}

fn editor_cmd(delta: &str, maybe_hunk: Option<&Hunk>) -> Command {
    let editor = std::env::var("EDITOR").expect("EDITOR not set");
    let mut cmd = Command::new(editor.clone());
    let args = match maybe_hunk {
        Some(hunk) => match editor.as_str() {
            "vi" | "vim" | "nvim" | "nano" => {
                vec![format!("+{}", hunk.new_start), delta.to_string()]
            }
            _ => vec![format!("{}:{}", delta, hunk.new_start)],
        },
        None => vec![delta.to_string()],
    };

    cmd.args(args);
    cmd
}

pub(crate) fn open_subscreen(
    terminal: &mut Terminal,
    input: &[u8],
    mut cmd: Command,
) -> Result<(), io::Error> {
    cmd.stdin(Stdio::piped());
    let mut cmd = cmd.spawn()?;

    use std::io::Write;
    cmd.stdin
        .take()
        .expect("Error taking stdin")
        .write_all(input)?;

    cmd.wait()?;

    terminal.hide_cursor()?;
    terminal.clear()?;

    Ok(())
}
