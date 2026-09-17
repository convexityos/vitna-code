//! Vitna TUI
//!
//! Terminal client for Vitna Code adhering to the Calm Terminal visual law:
//! - Dark only
//! - Dense but legible
//! - Zero faux chat bubbles, glassmorphism, gradients, or ambient glow
//! - Amber budget: maximum one attention signal per view

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

#[derive(Debug, Clone)]
pub struct TranscriptEntry {
    pub role: String,
    pub content: String,
    pub is_tool: bool,
}

#[derive(Debug, Clone)]
pub struct ApprovalPrompt {
    pub tool_name: String,
    pub action_digest: String,
    pub details: String,
}

pub struct TerminalApp {
    pub session_id: String,
    pub model_sku: String,
    pub isolation_label: String,
    pub input: String,
    pub transcript: Vec<TranscriptEntry>,
    pub active_diff: Option<String>,
    pub pending_approval: Option<ApprovalPrompt>,
    pub is_running: bool,
    pub turn_count: u32,
}

impl Default for TerminalApp {
    fn default() -> Self {
        Self {
            session_id: "sess-local-01".to_string(),
            model_sku: "claude-3-7-sonnet".to_string(),
            isolation_label: "Guarded".to_string(),
            input: String::new(),
            transcript: vec![
                TranscriptEntry {
                    role: "system".to_string(),
                    content: "Vitna Code ready. Calm Terminal mode active. Zero cloud transit.".to_string(),
                    is_tool: false,
                },
            ],
            active_diff: None,
            pending_approval: None,
            is_running: true,
            turn_count: 0,
        }
    }
}

impl TerminalApp {
    pub fn handle_key(&mut self, key: KeyEvent) {
        // Handle approval modal when pending
        if self.pending_approval.is_some() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.approve_pending();
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.reject_pending();
                }
                _ => {}
            }
            return;
        }

        // Standard navigation and typing
        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                self.is_running = false;
            }
            (_, KeyCode::Enter) => {
                let trimmed = self.input.trim().to_string();
                if !trimmed.is_empty() {
                    self.submit_prompt(trimmed);
                    self.input.clear();
                }
            }
            (_, KeyCode::Backspace) => {
                self.input.pop();
            }
            (_, KeyCode::Char(c)) => {
                self.input.push(c);
            }
            _ => {}
        }
    }

    pub fn submit_prompt(&mut self, prompt: String) {
        self.turn_count += 1;
        self.transcript.push(TranscriptEntry {
            role: "user".to_string(),
            content: prompt.clone(),
            is_tool: false,
        });

        // Simulate agent processing turn
        self.transcript.push(TranscriptEntry {
            role: "assistant".to_string(),
            content: format!("Analyzing request: '{}'. Inspecting workspace...", prompt),
            is_tool: false,
        });

        if prompt.contains("edit") || prompt.contains("write") || prompt.contains("add") {
            self.active_diff = Some(
                "--- a/config.toml\n+++ b/config.toml\n@@ -1,2 +1,3 @@\n [app]\n name = \"vitna\"\n+version = \"0.1.0\"\n".to_string(),
            );

            // Amber budget: present one approval request
            self.pending_approval = Some(ApprovalPrompt {
                tool_name: "write_file".to_string(),
                action_digest: "a7c8e9f2b1d304".to_string(),
                details: "Modify config.toml (+1 line)".to_string(),
            });
        }
    }

    pub fn approve_pending(&mut self) {
        if let Some(prompt) = self.pending_approval.take() {
            self.transcript.push(TranscriptEntry {
                role: "action".to_string(),
                content: format!("[APPROVED] Executed {} (digest: {})", prompt.tool_name, prompt.action_digest),
                is_tool: true,
            });
            self.transcript.push(TranscriptEntry {
                role: "receipt".to_string(),
                content: "Local receipt generated and signed: .vitna/receipts/run-latest.json".to_string(),
                is_tool: false,
            });
        }
    }

    pub fn reject_pending(&mut self) {
        if let Some(prompt) = self.pending_approval.take() {
            self.transcript.push(TranscriptEntry {
                role: "action".to_string(),
                content: format!("[REJECTED] Action {} cancelled by operator.", prompt.tool_name),
                is_tool: true,
            });
        }
    }

    pub fn render(&self, frame: &mut Frame) {
        let area = frame.area();

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Header
                Constraint::Min(8),    // Body (Transcript & Diff)
                Constraint::Length(3), // Input
            ])
            .split(area);

        self.render_header(frame, chunks[0]);
        self.render_body(frame, chunks[1]);
        self.render_input(frame, chunks[2]);

        if let Some(approval) = &self.pending_approval {
            self.render_approval_modal(frame, area, approval);
        }
    }

    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let header_text = vec![
            Line::from(vec![
                Span::styled("VITNA CODE", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
                Span::raw(" | Session: "),
                Span::styled(&self.session_id, Style::default().fg(Color::Cyan)),
                Span::raw(" | Model: "),
                Span::styled(&self.model_sku, Style::default().fg(Color::Green)),
                Span::raw(" | Sandbox: "),
                Span::styled(&self.isolation_label, Style::default().fg(Color::Magenta)),
                Span::raw(" | Turns: "),
                Span::styled(self.turn_count.to_string(), Style::default().fg(Color::Yellow)),
            ]),
            Line::from(vec![
                Span::styled("Honesty Contract: Zero Exfiltration | Hardware Attested Evidence | Signed Receipts", Style::default().fg(Color::DarkGray)),
            ]),
        ];

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" Convexity Terminal ");

        let paragraph = Paragraph::new(header_text).block(block);
        frame.render_widget(paragraph, area);
    }

    fn render_body(&self, frame: &mut Frame, area: Rect) {
        let body_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(60), // Transcript
                Constraint::Percentage(40), // Active Diff
            ])
            .split(area);

        // Transcript Panel
        let mut transcript_lines = Vec::new();
        for entry in &self.transcript {
            let role_style = match entry.role.as_str() {
                "system" => Style::default().fg(Color::DarkGray),
                "user" => Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                "assistant" => Style::default().fg(Color::White),
                "action" => Style::default().fg(Color::Green),
                "receipt" => Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
                _ => Style::default().fg(Color::Gray),
            };

            transcript_lines.push(Line::from(vec![
                Span::styled(format!("[{}] ", entry.role.to_uppercase()), role_style),
                Span::raw(&entry.content),
            ]));
        }

        let transcript_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" Evidence Ledger & Transcript ");

        let transcript_p = Paragraph::new(transcript_lines)
            .block(transcript_block)
            .wrap(Wrap { trim: false });
        frame.render_widget(transcript_p, body_chunks[0]);

        // Diff Panel
        let diff_content = self.active_diff.as_deref().unwrap_or("(No active diffs)");
        let diff_lines: Vec<Line> = diff_content
            .lines()
            .map(|l| {
                if l.starts_with('+') && !l.starts_with("+++") {
                    Line::from(Span::styled(l, Style::default().fg(Color::Green)))
                } else if l.starts_with('-') && !l.starts_with("---") {
                    Line::from(Span::styled(l, Style::default().fg(Color::Red)))
                } else if l.starts_with('@') {
                    Line::from(Span::styled(l, Style::default().fg(Color::Cyan)))
                } else {
                    Line::from(Span::styled(l, Style::default().fg(Color::DarkGray)))
                }
            })
            .collect();

        let diff_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" Changeset Diff Inspector ");

        let diff_p = Paragraph::new(diff_lines)
            .block(diff_block)
            .wrap(Wrap { trim: false });
        frame.render_widget(diff_p, body_chunks[1]);
    }

    fn render_input(&self, frame: &mut Frame, area: Rect) {
        let input_text = Line::from(vec![
            Span::styled("vitna> ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::raw(&self.input),
        ]);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" Prompt Input (Enter to submit, Ctrl+C to exit) ");

        let paragraph = Paragraph::new(input_text).block(block);
        frame.render_widget(paragraph, area);
    }

    fn render_approval_modal(&self, frame: &mut Frame, area: Rect, prompt: &ApprovalPrompt) {
        let modal_width = 60.min(area.width);
        let modal_height = 9.min(area.height);

        let modal_area = Rect {
            x: (area.width.saturating_sub(modal_width)) / 2,
            y: (area.height.saturating_sub(modal_height)) / 2,
            width: modal_width,
            height: modal_height,
        };

        frame.render_widget(Clear, modal_area);

        // Amber signal: Exactly one amber alert per view
        let amber_style = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);

        let lines = vec![
            Line::from(Span::styled("ACTION APPROVAL REQUIRED", amber_style)),
            Line::from(format!("Tool:   {}", prompt.tool_name)),
            Line::from(format!("Digest: {}", prompt.action_digest)),
            Line::from(format!("Detail: {}", prompt.details)),
            Line::from(""),
            Line::from(vec![
                Span::raw("Press "),
                Span::styled("[Y] Approve", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                Span::raw("  "),
                Span::styled("[N] Reject", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
            ]),
        ];

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(amber_style)
            .title(" Security Boundary ");

        let paragraph = Paragraph::new(lines).block(block);
        frame.render_widget(paragraph, modal_area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_terminal_app_lifecycle() {
        let mut app = TerminalApp::default();
        assert!(app.is_running);
        assert_eq!(app.turn_count, 0);

        // Submit task
        app.submit_prompt("Please inspect workspace".to_string());
        assert_eq!(app.turn_count, 1);
        assert_eq!(app.transcript.len(), 3);

        // Submit edit task triggering approval modal
        app.submit_prompt("Please write new file".to_string());
        assert!(app.pending_approval.is_some());
        assert!(app.active_diff.is_some());

        // Approve action
        app.approve_pending();
        assert!(app.pending_approval.is_none());
        assert!(app.transcript.iter().any(|t| t.content.contains("[APPROVED]")));
    }
}
