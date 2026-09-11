use std::collections::VecDeque;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use maki_agent::permissions::{
    DEFAULT_DENY_GUIDANCE, PermissionAnswer, RuleShape, proposed_rules, rule_shapes, scope_matches,
};
use maki_config::ToolKey;

use crate::components::Overlay;
use crate::components::form::render_form;
use crate::components::hint_line;
use crate::components::is_ctrl;
use crate::text_buffer::TextBuffer;
use crate::theme;

const HINT_ALLOW_ROW: &[(&str, &str)] = &[
    ("y", "Allow"),
    ("a", "Always (project)"),
    ("A", "Always (all projects)"),
    ("s", "Session"),
];
const HINT_DENY_ROW: &[(&str, &str)] = &[
    ("n", "Deny"),
    ("d", "Deny-always (project)"),
    ("D", "Deny-always (all)"),
];

// An untrusted folder keeps its project answers in memory, so the rows say
// what the answer really does instead of promising a saved rule.
const HINT_ALLOW_ROW_UNTRUSTED: &[(&str, &str)] = &[
    ("y", "Allow"),
    ("a", "Project (this session)"),
    ("A", "Always (all projects, saved)"),
    ("s", "Session"),
];
const HINT_DENY_ROW_UNTRUSTED: &[(&str, &str)] = &[
    ("n", "Deny"),
    ("d", "Deny project (this session)"),
    ("D", "Deny-always (all, saved)"),
];
const UNTRUSTED_NOTICE: &str = "folder not trusted, project answers last for this session only";
/// Nothing is written into a folder the user declined, so the answer that
/// survives a restart is the global one and the prompt has to name it.
const UNTRUSTED_DURABLE_ANSWER: &str = "use D to save a deny that outlives this session";

const CONFIRM_ALLOW_PROJECT_HINTS: &[(&str, &str)] = &[
    ("Enter / y", "Confirm allow-always (project)"),
    ("any", "Cancel"),
];
const CONFIRM_ALLOW_ALL_HINTS: &[(&str, &str)] = &[
    ("Enter / y", "Confirm allow-always (all projects)"),
    ("any", "Cancel"),
];
const CONFIRM_SESSION_HINTS: &[(&str, &str)] =
    &[("Enter / y", "Confirm allow (session)"), ("any", "Cancel")];
const CONFIRM_DENY_PROJECT_HINTS: &[(&str, &str)] = &[
    ("Enter / y", "Confirm deny-always (project)"),
    ("any", "Cancel"),
];
const CONFIRM_DENY_ALL_HINTS: &[(&str, &str)] = &[
    ("Enter / y", "Confirm deny-always (all projects)"),
    ("any", "Cancel"),
];
const CONFIRM_ALLOW_PROJECT_SESSION_HINTS: &[(&str, &str)] = &[
    ("Enter / y", "Confirm allow (project, this session)"),
    ("any", "Cancel"),
];
const CONFIRM_DENY_PROJECT_SESSION_HINTS: &[(&str, &str)] = &[
    ("Enter / y", "Confirm deny (project, this session)"),
    ("any", "Cancel"),
];

const DENY_GUIDANCE_HINTS: &[(&str, &str)] = &[("Enter", "Deny"), ("Esc", "Cancel")];
const QUEUED_NOTICE: &str = "more request(s) waiting";
const RULE_EDIT_HINTS: &[(&str, &str)] = &[
    ("Enter", "Accept rules"),
    ("Esc", "Revert"),
    ("↑/↓", "Rule"),
    ("Tab", "Done — answer as usual"),
];
const SHAPE_HINT: (&str, &str) = ("←/→", "Rule shape");
const EDIT_HINT: (&str, &str) = ("e", "Edit rules");
const EDIT_CUE: &str = " (e edits)";
const CUSTOM_SHAPE_LABEL: &str = "custom";
const MATCH_MARK: &str = "✓ ";
const MISS_MARK: &str = "✗ ";

fn shape_label(shape: RuleShape) -> &'static str {
    match shape {
        RuleShape::Exact => "exact",
        RuleShape::Wildcard => "wildcard",
        RuleShape::PathScoped => "path-scoped",
    }
}

fn cycled_shape(tool: &ToolKey, current: RuleShape, forward: bool) -> RuleShape {
    let shapes = rule_shapes(tool);
    let index = shapes.iter().position(|s| *s == current).unwrap_or(0);
    let step = if forward { 1 } else { shapes.len() - 1 };
    shapes[(index + step) % shapes.len()]
}

fn confirm_hints(base: &[(&str, &str)], offers_shapes: bool) -> Line<'static> {
    let mut hints = base.to_vec();
    if offers_shapes {
        hints.push(SHAPE_HINT);
    }
    hints.push(EDIT_HINT);
    hint_line(&hints)
}

fn caret_spans(text: &str, cursor: usize, style: Style) -> Vec<Span<'static>> {
    let (before, after) = text.split_at(TextBuffer::char_to_byte(text, cursor));
    let mut chars = after.chars();
    let cursor_ch = chars.next().unwrap_or(' ');
    let rest: String = chars.collect();
    let mut spans = Vec::with_capacity(3);
    if !before.is_empty() {
        spans.push(Span::styled(before.to_string(), style));
    }
    spans.push(Span::styled(cursor_ch.to_string(), Style::new().reversed()));
    if !rest.is_empty() {
        spans.push(Span::styled(rest, style));
    }
    spans
}

pub(crate) struct RuleEditor {
    rows: Vec<TextBuffer>,
    selected: usize,
    /// Done state: the prompt's own answer keys pass through and typing goes
    /// back to editing, so the editor is a second entrypoint to the same
    /// answers rather than a detour.
    locked: bool,
}

impl RuleEditor {
    fn over(rules: &[String]) -> Self {
        let rows = rules
            .iter()
            .map(|rule| {
                let mut row = TextBuffer::new(rule.clone());
                row.move_to_end();
                row
            })
            .collect();
        Self {
            rows,
            selected: 0,
            locked: false,
        }
    }

    fn rules(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|row| row.value().trim().to_string())
            .filter(|rule| !rule.is_empty())
            .collect()
    }

    fn handle_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => {
                self.selected = (self.selected + 1).min(self.rows.len().saturating_sub(1));
            }
            _ => {
                if let Some(row) = self.rows.get_mut(self.selected) {
                    row.handle_key(key);
                }
            }
        }
    }

    fn insert_text(&mut self, text: &str) {
        if let Some(row) = self.rows.get_mut(self.selected) {
            row.insert_text(text);
        }
    }
}

fn aligned_hint_rows(rows: &[&[(&str, &str)]]) -> Vec<Line<'static>> {
    let t = theme::current();
    let max_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut col_widths = vec![0usize; max_cols];
    for row in rows {
        for (i, (key, desc)) in row.iter().enumerate() {
            let cell_len = key.len() + 1 + desc.len();
            col_widths[i] = col_widths[i].max(cell_len);
        }
    }
    rows.iter()
        .map(|row| {
            let mut spans = Vec::with_capacity(row.len() * 2);
            for (i, (key, desc)) in row.iter().enumerate() {
                spans.push(Span::styled(format!("  {key}"), t.keybind_key));
                let cell_len = key.len() + 1 + desc.len();
                let pad = if i + 1 < row.len() {
                    col_widths[i].saturating_sub(cell_len)
                } else {
                    0
                };
                spans.push(Span::styled(
                    format!(" {desc}{:width$}", "", width = pad),
                    t.tool_dim,
                ));
            }
            Line::from(spans)
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum PromptState {
    #[default]
    Normal,
    ConfirmAllowAlwaysProject,
    ConfirmAllowAlwaysGlobal,
    ConfirmAllowSession,
    ConfirmDenyAlwaysProject,
    ConfirmDenyAlwaysGlobal,
    DenyEditing,
}

impl PromptState {
    fn confirms_allow_rules(self) -> bool {
        matches!(
            self,
            Self::ConfirmAllowAlwaysProject
                | Self::ConfirmAllowAlwaysGlobal
                | Self::ConfirmAllowSession
        )
    }
}

struct Request {
    id: String,
    tool: ToolKey,
    scopes: Vec<String>,
    subagent_id: Option<String>,
    project_trusted: bool,
    shape: RuleShape,
    rules: Vec<String>,
    rule_editor: Option<Box<RuleEditor>>,
}

/// An answer carries the ask it settles: the agent only accepts one naming the
/// request it is parked on, and the ask on screen is not always the last one in.
#[derive(Debug)]
pub struct AnsweredRequest {
    pub id: String,
    pub subagent_id: Option<String>,
    pub answer: PermissionAnswer,
}

/// Every request parks a tool call until it is answered, so asks that arrive
/// while one is on screen queue up behind it instead of replacing it.
pub struct PermissionPrompt {
    /// The front is the one on screen; `state` and `buffer` belong to it and
    /// reset whenever it leaves.
    queue: VecDeque<Request>,
    state: PromptState,
    buffer: TextBuffer,
}

impl Overlay for PermissionPrompt {
    fn is_open(&self) -> bool {
        !self.queue.is_empty()
    }

    fn is_modal(&self) -> bool {
        false
    }

    /// Only reached when the run those asks belonged to is gone (cancel,
    /// session switch), which is what unparks their tool calls anyway.
    fn close(&mut self) {
        self.queue.clear();
        self.reset_entry();
    }
}

impl PermissionPrompt {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            state: PromptState::Normal,
            buffer: TextBuffer::new(String::new()),
        }
    }

    pub fn push(
        &mut self,
        id: String,
        tool: ToolKey,
        scopes: Vec<String>,
        subagent_id: Option<String>,
        project_trusted: bool,
    ) {
        let shape = RuleShape::default();
        let rules = proposed_rules(&tool, &scopes, shape);
        self.queue.push_back(Request {
            id,
            tool,
            scopes,
            subagent_id,
            project_trusted,
            shape,
            rules,
            rule_editor: None,
        });
    }

    fn reset_entry(&mut self) {
        self.state = PromptState::Normal;
        self.buffer = TextBuffer::new(String::new());
    }

    /// A cancelled subagent has nothing left to answer with, so its asks leave
    /// with it instead of parking the panel on a dead tool call.
    pub fn drop_subagent(&mut self, subagent_id: &str) {
        let owns = |request: &Request| request.subagent_id.as_deref() == Some(subagent_id);
        if self.queue.front().is_some_and(owns) {
            self.reset_entry();
        }
        self.queue.retain(|request| !owns(request));
    }

    pub(crate) fn tool(&self) -> Option<&ToolKey> {
        self.queue.front().map(|request| &request.tool)
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<AnsweredRequest> {
        let answer = self.key_answer(key)?;
        let request = self.queue.pop_front()?;
        self.reset_entry();
        Some(AnsweredRequest {
            id: request.id,
            subagent_id: request.subagent_id,
            answer,
        })
    }

    fn key_answer(&mut self, key: KeyEvent) -> Option<PermissionAnswer> {
        if !self.is_open() {
            return None;
        }
        let Request {
            tool,
            scopes,
            shape,
            rules,
            rule_editor,
            ..
        } = self.queue.front_mut()?;
        let (state, buffer) = (&mut self.state, &mut self.buffer);
        if is_ctrl(&key) && key.code == KeyCode::Char('c') {
            return Some(PermissionAnswer::Deny);
        }
        if let Some(editor) = rule_editor.as_mut() {
            if editor.locked {
                match key.code {
                    KeyCode::Tab => {
                        editor.locked = false;
                        return None;
                    }
                    KeyCode::Up | KeyCode::Down => {
                        editor.handle_key(key);
                        return None;
                    }
                    KeyCode::Esc => {
                        *rules = proposed_rules(tool, scopes, *shape);
                        *rule_editor = None;
                        return None;
                    }
                    KeyCode::Enter => {
                        *rules = editor.rules();
                        *rule_editor = None;
                        return None;
                    }
                    KeyCode::Char('y' | 'n' | 'a' | 'A' | 's' | 'd' | 'D') => {
                        *rules = editor.rules();
                        *rule_editor = None;
                        // fall through: the prompt answers this key as if the
                        // editor were never open
                    }
                    _ => {
                        editor.locked = false;
                        editor.handle_key(key);
                        return None;
                    }
                }
            } else {
                let settled = match key.code {
                    KeyCode::Enter => Some(editor.rules()),
                    KeyCode::Esc => Some(proposed_rules(tool, scopes, *shape)),
                    KeyCode::Tab => {
                        editor.locked = true;
                        None
                    }
                    _ => {
                        editor.handle_key(key);
                        None
                    }
                };
                if let Some(list) = settled {
                    *rules = list;
                    *rule_editor = None;
                }
                return None;
            }
        }
        if *state == PromptState::DenyEditing {
            return match key.code {
                KeyCode::Enter => {
                    let text = buffer.value().trim().to_string();
                    if text.is_empty() {
                        Some(PermissionAnswer::Deny)
                    } else {
                        Some(PermissionAnswer::DenyWithGuidance(text))
                    }
                }
                KeyCode::Esc => {
                    *buffer = TextBuffer::new(String::new());
                    *state = PromptState::Normal;
                    None
                }
                _ => {
                    buffer.handle_key(key);
                    None
                }
            };
        }
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return None;
        }
        let offers_shapes = rule_shapes(tool).len() > 1;
        if state.confirms_allow_rules() || *state == PromptState::Normal {
            match key.code {
                KeyCode::Right | KeyCode::Left => {
                    *shape = cycled_shape(tool, *shape, key.code == KeyCode::Right);
                    *rules = proposed_rules(tool, scopes, *shape);
                    return None;
                }
                KeyCode::Char('e') if offers_shapes => {
                    *rule_editor = Some(Box::new(RuleEditor::over(rules)));
                    return None;
                }
                _ => {}
            }
        }
        let confirm_answer = match *state {
            PromptState::ConfirmAllowAlwaysProject => Some(PermissionAnswer::AllowAlwaysProject {
                rules: rules.clone(),
            }),
            PromptState::ConfirmAllowAlwaysGlobal => Some(PermissionAnswer::AllowAlwaysGlobal {
                rules: rules.clone(),
            }),
            PromptState::ConfirmAllowSession => Some(PermissionAnswer::AllowSession {
                rules: rules.clone(),
            }),
            PromptState::ConfirmDenyAlwaysProject => Some(PermissionAnswer::DenyAlwaysProject),
            PromptState::ConfirmDenyAlwaysGlobal => Some(PermissionAnswer::DenyAlwaysGlobal),
            _ => None,
        };
        if let Some(answer) = confirm_answer {
            return match key.code {
                KeyCode::Char('y') | KeyCode::Enter => Some(answer),
                _ => {
                    let hand_edited = rule_shapes(tool)
                        .iter()
                        .all(|s| proposed_rules(tool, scopes, *s) != *rules);
                    *state = PromptState::Normal;
                    *shape = RuleShape::default();
                    if !hand_edited {
                        *rules = proposed_rules(tool, scopes, *shape);
                    }
                    None
                }
            };
        }
        match key.code {
            KeyCode::Char('y') => Some(PermissionAnswer::AllowOnce),
            KeyCode::Char('n') => {
                *state = PromptState::DenyEditing;
                None
            }
            KeyCode::Char('a') => {
                *state = PromptState::ConfirmAllowAlwaysProject;
                None
            }
            KeyCode::Char('A') => {
                *state = PromptState::ConfirmAllowAlwaysGlobal;
                None
            }
            KeyCode::Char('d') => {
                *state = PromptState::ConfirmDenyAlwaysProject;
                None
            }
            KeyCode::Char('D') => {
                *state = PromptState::ConfirmDenyAlwaysGlobal;
                None
            }
            KeyCode::Char('s') => {
                *state = PromptState::ConfirmAllowSession;
                None
            }
            _ => None,
        }
    }

    pub fn handle_paste(&mut self, text: &str) -> bool {
        if !self.is_open() {
            return false;
        }
        let editor = self
            .queue
            .front_mut()
            .and_then(|request| request.rule_editor.as_mut());
        if let Some(editor) = editor {
            editor.locked = false;
            editor.insert_text(text);
            return true;
        }
        if self.state != PromptState::DenyEditing {
            return false;
        }
        self.buffer.insert_text(text);
        true
    }

    fn build_lines(&self) -> Vec<Line<'static>> {
        let Some(Request {
            tool,
            scopes,
            subagent_id,
            project_trusted,
            shape,
            rules,
            rule_editor,
            ..
        }) = self.queue.front()
        else {
            return vec![];
        };
        let (state, buffer) = (&self.state, &self.buffer);
        let picking_shape = state.confirms_allow_rules();
        let previewing = picking_shape || rule_editor.is_some();
        let offers_shapes = rule_shapes(tool).len() > 1;
        let live_rules = match rule_editor {
            Some(editor) => editor.rules(),
            None => rules.clone(),
        };
        let t = theme::current();
        let label_style = t.tool_dim;
        let value_style = Style::new().fg(t.foreground);

        let mut tool_spans = vec![Span::raw("  "), Span::styled("tool  ", label_style)];
        if subagent_id.is_some() {
            tool_spans.push(Span::styled("[subtask] ", t.item_desc));
        }
        tool_spans.push(Span::styled(tool.to_string(), value_style));

        let mut lines = vec![Line::raw(""), Line::from(tool_spans)];
        let waiting = self.queue.len() - 1;
        if waiting > 0 {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled("queue ", label_style),
                Span::styled(format!("{waiting} {QUEUED_NOTICE}"), t.item_desc),
            ]));
        }
        for (i, s) in scopes.iter().enumerate() {
            let label = if i == 0 { "scope " } else { "    + " };
            let mut spans = vec![Span::raw("  "), Span::styled(label, label_style)];
            if previewing {
                let (mark, style) = if live_rules.iter().any(|rule| scope_matches(rule, s)) {
                    (MATCH_MARK, t.tool_success)
                } else {
                    (MISS_MARK, t.tool_error)
                };
                spans.push(Span::styled(mark, style));
                spans.push(Span::styled(s.clone(), style));
            } else {
                spans.push(Span::styled(s.clone(), value_style));
            }
            lines.push(Line::from(spans));
        }

        let allow_label = |i: usize| if i == 0 { "allow " } else { "    + " };
        match rule_editor {
            Some(editor) => {
                for (i, row) in editor.rows.iter().enumerate() {
                    let mut spans =
                        vec![Span::raw("  "), Span::styled(allow_label(i), label_style)];
                    if !editor.locked && i == editor.selected {
                        spans.extend(caret_spans(&row.value(), row.x(), value_style));
                    } else {
                        spans.push(Span::styled(row.value(), value_style));
                    }
                    lines.push(Line::from(spans));
                }
            }
            None if picking_shape || *rules != *scopes => {
                let cue = *state == PromptState::Normal && offers_shapes;
                let last = rules.len().saturating_sub(1);
                for (i, rule) in rules.iter().enumerate() {
                    let mut spans = vec![
                        Span::raw("  "),
                        Span::styled(allow_label(i), label_style),
                        Span::styled(rule.clone(), value_style),
                    ];
                    if cue && i == last {
                        spans.push(Span::styled(EDIT_CUE, t.tool_dim));
                    }
                    lines.push(Line::from(spans));
                }
            }
            None => {}
        }
        if picking_shape && offers_shapes && rule_editor.is_none() {
            let label = if *rules == proposed_rules(tool, scopes, *shape) {
                shape_label(*shape)
            } else {
                CUSTOM_SHAPE_LABEL
            };
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled("shape ", label_style),
                Span::styled(label, value_style),
            ]));
        }

        if *state == PromptState::DenyEditing {
            let text = buffer.value();
            let mut spans = vec![Span::raw("  "), Span::styled("guide ", label_style)];
            if text.is_empty() {
                spans.extend(caret_spans(DEFAULT_DENY_GUIDANCE, 0, t.tool_dim));
            } else {
                spans.extend(caret_spans(&text, buffer.x(), Style::new()));
            }
            lines.push(Line::from(spans));
        }

        if !*project_trusted {
            for notice in [UNTRUSTED_NOTICE, UNTRUSTED_DURABLE_ANSWER] {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(notice, t.tool_dim),
                ]));
            }
        }

        lines.push(Line::raw(""));
        if let Some(editor) = &rule_editor {
            if editor.locked {
                let rows = if *project_trusted {
                    [HINT_ALLOW_ROW, HINT_DENY_ROW]
                } else {
                    [HINT_ALLOW_ROW_UNTRUSTED, HINT_DENY_ROW_UNTRUSTED]
                };
                lines.extend(aligned_hint_rows(&rows));
            } else {
                lines.push(hint_line(RULE_EDIT_HINTS));
            }
            lines.push(Line::raw(""));
            return lines;
        }
        match *state {
            PromptState::ConfirmAllowAlwaysProject => {
                let base = if *project_trusted {
                    CONFIRM_ALLOW_PROJECT_HINTS
                } else {
                    CONFIRM_ALLOW_PROJECT_SESSION_HINTS
                };
                lines.push(confirm_hints(base, offers_shapes));
            }
            PromptState::ConfirmAllowAlwaysGlobal => {
                lines.push(confirm_hints(CONFIRM_ALLOW_ALL_HINTS, offers_shapes));
            }
            PromptState::ConfirmAllowSession => {
                lines.push(confirm_hints(CONFIRM_SESSION_HINTS, offers_shapes));
            }
            PromptState::ConfirmDenyAlwaysProject => {
                lines.push(hint_line(if *project_trusted {
                    CONFIRM_DENY_PROJECT_HINTS
                } else {
                    CONFIRM_DENY_PROJECT_SESSION_HINTS
                }));
            }
            PromptState::ConfirmDenyAlwaysGlobal => {
                lines.push(hint_line(CONFIRM_DENY_ALL_HINTS));
            }
            PromptState::DenyEditing => {
                lines.push(hint_line(DENY_GUIDANCE_HINTS));
            }
            PromptState::Normal => {
                let rows = if *project_trusted {
                    [HINT_ALLOW_ROW, HINT_DENY_ROW]
                } else {
                    [HINT_ALLOW_ROW_UNTRUSTED, HINT_DENY_ROW_UNTRUSTED]
                };
                lines.extend(aligned_hint_rows(&rows));
            }
        }
        lines.push(Line::raw(""));
        lines
    }

    pub fn view(&self, frame: &mut Frame, area: Rect) {
        if !self.is_open() {
            return;
        }
        let lines = self.build_lines();
        let t = theme::current();
        render_form(&t, " Permission Required ", frame, area, lines, (0, 0));
    }

    pub fn height(&self, width: u16) -> u16 {
        let inner_width = width.saturating_sub(2);
        let lines = self.build_lines();
        let para = Paragraph::new(lines).wrap(Wrap { trim: false });
        para.line_count(inner_width) as u16 + 2
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use maki_agent::permissions::PermissionAnswer;
    use maki_config::ToolKey;
    use test_case::test_case;

    use super::{
        CONFIRM_ALLOW_PROJECT_HINTS, CONFIRM_ALLOW_PROJECT_SESSION_HINTS,
        CONFIRM_DENY_PROJECT_HINTS, CONFIRM_DENY_PROJECT_SESSION_HINTS, EDIT_CUE, MATCH_MARK,
        MISS_MARK, Overlay, PermissionPrompt, PromptState, QUEUED_NOTICE, UNTRUSTED_DURABLE_ANSWER,
        UNTRUSTED_NOTICE,
    };

    const MAIN_ID: &str = "id";
    const SUB_ID: &str = "id-2";
    const SUB_AGENT: &str = "sub-2";
    const CD_SCOPE: &str = "cd /repo";
    const URL_SCOPE: &str = "https://example.com";

    fn open_with(tool: ToolKey, scope: &str) -> PermissionPrompt {
        let mut prompt = PermissionPrompt::new();
        prompt.push(MAIN_ID.into(), tool, vec![scope.into()], None, true);
        prompt
    }

    fn open_trusted(prompt: &mut PermissionPrompt, project_trusted: bool) {
        prompt.push(
            MAIN_ID.into(),
            ToolKey::native("bash"),
            vec!["execute".into()],
            None,
            project_trusted,
        );
    }

    fn push_subagent_ask(prompt: &mut PermissionPrompt) {
        prompt.push(
            SUB_ID.into(),
            ToolKey::native("read"),
            vec!["/tmp/x".into()],
            Some(SUB_AGENT.into()),
            true,
        );
    }

    /// Most tests only care about the answer, not the ask it came back with.
    fn answer(prompt: &mut PermissionPrompt, key: KeyEvent) -> Option<PermissionAnswer> {
        prompt.handle_key(key).map(|answered| answered.answer)
    }

    fn open_prompt() -> PermissionPrompt {
        open_with(ToolKey::native("bash"), "execute")
    }

    fn rendered(prompt: &PermissionPrompt) -> String {
        prompt
            .build_lines()
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn state_of(prompt: &PermissionPrompt) -> PromptState {
        assert!(prompt.is_open(), "expected an open prompt");
        prompt.state
    }

    fn rules_of(answer: Option<PermissionAnswer>) -> Vec<String> {
        match answer {
            Some(
                PermissionAnswer::AllowSession { rules }
                | PermissionAnswer::AllowAlwaysProject { rules }
                | PermissionAnswer::AllowAlwaysGlobal { rules },
            ) => rules,
            other => panic!("expected a rule-writing allow, got {other:?}"),
        }
    }

    fn press(prompt: &mut PermissionPrompt, codes: &[KeyCode]) {
        for code in codes {
            assert!(
                prompt.handle_key(key(*code)).is_none(),
                "{code:?} answered early"
            );
        }
    }

    #[test]
    fn edit_opens_from_the_initial_prompt() {
        let mut prompt = open_prompt();
        press(&mut prompt, &[KeyCode::Char('e')]);
        assert!(editing(&prompt));
    }

    #[test]
    fn done_editor_answers_with_the_prompt_keys() {
        let mut prompt = open_with(ToolKey::native("bash"), CD_SCOPE);
        press(
            &mut prompt,
            &[KeyCode::Char('e'), KeyCode::Tab, KeyCode::Char('s')],
        );
        assert_eq!(state_of(&prompt), PromptState::ConfirmAllowSession);
        assert_eq!(
            rules_of(answer(&mut prompt, key(KeyCode::Enter))),
            vec!["cd *"]
        );
    }

    #[test]
    fn y_from_a_done_editor_answers_allow_once() {
        let mut prompt = open_prompt();
        press(&mut prompt, &[KeyCode::Char('e'), KeyCode::Tab]);
        assert_eq!(
            answer(&mut prompt, key(KeyCode::Char('y'))),
            Some(PermissionAnswer::AllowOnce)
        );
    }

    #[test]
    fn typing_from_a_done_editor_resumes_editing() {
        let mut prompt = open_prompt();
        press(
            &mut prompt,
            &[KeyCode::Char('e'), KeyCode::Tab, KeyCode::Char('x')],
        );
        let editor = prompt
            .queue
            .front()
            .and_then(|request| request.rule_editor.as_ref());
        assert!(
            editor.is_some_and(|editor| !editor.locked),
            "expected the editor to stay open"
        );
        assert!(rendered(&prompt).contains("execute *x"));
    }

    #[test]
    fn initial_prompt_cues_editing_for_shape_tools_only() {
        assert!(rendered(&open_prompt()).contains(EDIT_CUE));
        let mcp = open_with(ToolKey::parse("srv.tool").unwrap(), URL_SCOPE);
        assert!(!rendered(&mcp).contains(EDIT_CUE));
    }

    #[test]
    fn edited_rules_survive_a_cancelled_confirm() {
        let mut prompt = open_prompt();
        press(
            &mut prompt,
            &[KeyCode::Char('e'), KeyCode::Char('X'), KeyCode::Enter],
        );
        press(&mut prompt, &[KeyCode::Char('s'), KeyCode::Char('q')]);
        assert_eq!(state_of(&prompt), PromptState::Normal);
        assert!(rendered(&prompt).contains("execute *X"));
    }

    #[test]
    fn shape_cycled_rules_reset_on_cancelled_confirm() {
        let mut prompt = open_with(ToolKey::native("bash"), CD_SCOPE);
        press(
            &mut prompt,
            &[KeyCode::Right, KeyCode::Char('s'), KeyCode::Char('q')],
        );
        assert_eq!(state_of(&prompt), PromptState::Normal);
        assert!(rendered(&prompt).contains("cd *"));
    }

    fn ctrl_c() -> KeyEvent {
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn ctrl_c_denies() {
        let mut prompt = open_prompt();
        assert_eq!(answer(&mut prompt, ctrl_c()), Some(PermissionAnswer::Deny));
        // Also test from editing state
        let mut prompt2 = open_prompt();
        prompt2.handle_key(key(KeyCode::Char('n')));
        prompt2.handle_key(key(KeyCode::Char('t')));
        assert_eq!(answer(&mut prompt2, ctrl_c()), Some(PermissionAnswer::Deny));
    }

    #[test]
    fn n_goes_to_deny_editing() {
        let mut prompt = open_prompt();
        assert_eq!(answer(&mut prompt, key(KeyCode::Char('n'))), None);
        assert_eq!(prompt.state, PromptState::DenyEditing);
    }

    #[test]
    fn deny_editing_esc_returns_to_normal() {
        let mut prompt = open_prompt();
        prompt.handle_key(key(KeyCode::Char('n')));
        prompt.handle_key(key(KeyCode::Char('t')));
        assert_eq!(answer(&mut prompt, key(KeyCode::Esc)), None);
        assert_eq!(prompt.state, PromptState::Normal);
        assert!(prompt.buffer.value().is_empty());
    }

    #[test]
    fn deny_editing_enter_empty_sends_deny() {
        let mut prompt = open_prompt();
        prompt.handle_key(key(KeyCode::Char('n')));
        assert_eq!(
            answer(&mut prompt, key(KeyCode::Enter)),
            Some(PermissionAnswer::Deny)
        );
    }

    #[test]
    fn deny_editing_with_text_sends_guidance() {
        let mut prompt = open_prompt();
        prompt.handle_key(key(KeyCode::Char('n')));
        prompt.handle_paste("Use cat");
        assert_eq!(
            answer(&mut prompt, key(KeyCode::Enter)),
            Some(PermissionAnswer::DenyWithGuidance("Use cat".into()))
        );
    }

    #[test]
    fn handle_paste_requires_editing_mode() {
        let mut prompt = open_prompt();
        assert!(!prompt.handle_paste("ignored"));
        prompt.handle_key(key(KeyCode::Char('n')));
        assert!(prompt.handle_paste("accepted"));
        assert_eq!(prompt.buffer.value(), "accepted");
    }

    #[test_case('s' => matches Some(PermissionAnswer::AllowSession { .. }) ; "s_is_session")]
    #[test_case('a' => matches Some(PermissionAnswer::AllowAlwaysProject { .. }) ; "a_is_project")]
    #[test_case('A' => matches Some(PermissionAnswer::AllowAlwaysGlobal { .. }) ; "shift_a_is_global")]
    fn confirm_key_picks_the_variant(open: char) -> Option<PermissionAnswer> {
        let mut prompt = open_prompt();
        press(&mut prompt, &[KeyCode::Char(open)]);
        answer(&mut prompt, key(KeyCode::Enter))
    }

    /// The rules in the answer are the rules on screen: the default shape
    /// writes what every approval wrote before, and each `Right` walks the
    /// shapes the core offers for bash until it wraps.
    #[test_case('s', 0 => vec!["cd *"] ; "default_is_wildcard")]
    #[test_case('a', 1 => vec!["cd /repo"] ; "one_right_is_exact")]
    #[test_case('A', 2 => vec!["cd /repo", "cd /repo/*"] ; "two_rights_is_path_scoped")]
    #[test_case('s', 3 => vec!["cd *"] ; "three_rights_wrap")]
    fn confirm_commits_the_displayed_rules(open: char, rights: usize) -> Vec<String> {
        let mut prompt = open_with(ToolKey::native("bash"), CD_SCOPE);
        press(&mut prompt, &[KeyCode::Char(open)]);
        press(&mut prompt, &vec![KeyCode::Right; rights]);
        assert_ne!(state_of(&prompt), PromptState::Normal);
        rules_of(answer(&mut prompt, key(KeyCode::Char('y'))))
    }

    #[test]
    fn left_cycles_backwards() {
        let mut prompt = open_with(ToolKey::native("bash"), CD_SCOPE);
        press(&mut prompt, &[KeyCode::Char('s'), KeyCode::Left]);
        assert_eq!(
            rules_of(answer(&mut prompt, key(KeyCode::Enter))),
            vec!["cd /repo", "cd /repo/*"]
        );
    }

    #[test]
    fn cancelled_confirm_forgets_the_shape() {
        let mut prompt = open_with(ToolKey::native("bash"), CD_SCOPE);
        press(
            &mut prompt,
            &[KeyCode::Char('s'), KeyCode::Right, KeyCode::Esc],
        );
        assert_eq!(state_of(&prompt), PromptState::Normal);
        press(&mut prompt, &[KeyCode::Char('a')]);
        assert_eq!(
            rules_of(answer(&mut prompt, key(KeyCode::Enter))),
            vec!["cd *"]
        );
    }

    #[test]
    fn fixed_shape_tool_ignores_cycling() {
        let mut prompt = open_with(ToolKey::native("webfetch"), URL_SCOPE);
        press(
            &mut prompt,
            &[KeyCode::Char('s'), KeyCode::Right, KeyCode::Left],
        );
        assert_eq!(state_of(&prompt), PromptState::ConfirmAllowSession);
        assert_eq!(
            rules_of(answer(&mut prompt, key(KeyCode::Enter))),
            vec![URL_SCOPE]
        );
    }

    #[test]
    fn deny_confirm_does_not_cycle() {
        let mut prompt = open_with(ToolKey::native("bash"), CD_SCOPE);
        press(&mut prompt, &[KeyCode::Char('d'), KeyCode::Right]);
        assert_eq!(state_of(&prompt), PromptState::Normal);
    }

    #[test]
    fn wildcard_tool_key_opens() {
        let mut prompt = PermissionPrompt::new();
        prompt.push(MAIN_ID.into(), ToolKey::Wildcard, vec![], None, true);
        assert!(prompt.is_open());
    }

    /// Parallel subagents each park a tool call on their own ask. An ask that
    /// lands while another is on screen has to wait its turn, not replace it:
    /// the replaced one would never be answered and its tool call would hang.
    #[test]
    fn a_second_request_waits_behind_the_one_on_screen() {
        let mut prompt = open_prompt();
        push_subagent_ask(&mut prompt);

        assert!(rendered(&prompt).contains(QUEUED_NOTICE));

        prompt.handle_key(key(KeyCode::Char('n')));
        prompt.handle_paste("main only");
        let first = prompt.handle_key(key(KeyCode::Enter)).expect("an answer");
        assert_eq!(first.id, MAIN_ID);
        assert_eq!(first.subagent_id, None);
        assert_eq!(
            first.answer,
            PermissionAnswer::DenyWithGuidance("main only".into())
        );

        assert!(prompt.is_open(), "the queued ask takes the panel");
        assert!(!rendered(&prompt).contains(QUEUED_NOTICE));

        // 'y' only answers if the half-typed deny left with the ask it was typed
        // for, otherwise it is still landing in the buffer.
        let second = prompt
            .handle_key(key(KeyCode::Char('y')))
            .expect("an answer");
        assert_eq!(second.id, SUB_ID);
        assert_eq!(second.subagent_id.as_deref(), Some(SUB_AGENT));
        assert_eq!(second.answer, PermissionAnswer::AllowOnce);
        assert!(!prompt.is_open());
    }

    #[test]
    fn a_cancelled_subagent_takes_its_request_with_it() {
        let mut prompt = PermissionPrompt::new();
        push_subagent_ask(&mut prompt);
        open_trusted(&mut prompt, true);
        prompt.handle_key(key(KeyCode::Char('n')));

        prompt.drop_subagent(SUB_AGENT);

        let answered = prompt
            .handle_key(key(KeyCode::Char('y')))
            .expect("the ask behind it moves up with a clean entry state");
        assert_eq!(answered.id, MAIN_ID);
        assert!(!prompt.is_open());
    }

    /// The always-answers are scoped to the project, and the hint has to say so
    /// before the user commits to one. In an untrusted folder the answer only
    /// holds for the session, and the hint says that instead.
    #[test_case(KeyCode::Char('a'), true, PromptState::ConfirmAllowAlwaysProject, CONFIRM_ALLOW_PROJECT_HINTS ; "allow_always")]
    #[test_case(KeyCode::Char('d'), true, PromptState::ConfirmDenyAlwaysProject, CONFIRM_DENY_PROJECT_HINTS ; "deny_always")]
    #[test_case(KeyCode::Char('a'), false, PromptState::ConfirmAllowAlwaysProject, CONFIRM_ALLOW_PROJECT_SESSION_HINTS ; "allow_always_untrusted")]
    #[test_case(KeyCode::Char('d'), false, PromptState::ConfirmDenyAlwaysProject, CONFIRM_DENY_PROJECT_SESSION_HINTS ; "deny_always_untrusted")]
    fn project_answers_confirm_before_they_apply(
        code: KeyCode,
        project_trusted: bool,
        expected: PromptState,
        hints: &[(&str, &str)],
    ) {
        let mut prompt = PermissionPrompt::new();
        open_trusted(&mut prompt, project_trusted);

        assert_eq!(answer(&mut prompt, key(code)), None);

        assert_eq!(prompt.state, expected);
        let text = rendered(&prompt);
        assert!(text.contains(hints[0].1), "hint missing from: {text}");
    }

    /// The project answers stay on the screen in an untrusted folder, so the
    /// prompt has to explain how long they last and where a lasting deny goes
    /// instead.
    #[test_case(true, false ; "trusted_folder_says_nothing")]
    #[test_case(false, true ; "untrusted_folder_warns")]
    fn untrusted_folder_notice_follows_trust(project_trusted: bool, warned: bool) {
        let mut prompt = PermissionPrompt::new();
        open_trusted(&mut prompt, project_trusted);

        let text = rendered(&prompt);
        assert_eq!(text.contains(UNTRUSTED_NOTICE), warned, "rendered: {text}");
        assert_eq!(
            text.contains(UNTRUSTED_DURABLE_ANSWER),
            warned,
            "rendered: {text}"
        );
        assert_eq!(
            answer(&mut prompt, key(KeyCode::Char('a'))),
            None,
            "the project answer stays available either way"
        );
    }

    const LS_SCOPE: &str = "ls -la";

    fn open_two_scopes() -> PermissionPrompt {
        let mut prompt = PermissionPrompt::new();
        prompt.push(
            MAIN_ID.into(),
            ToolKey::native("bash"),
            vec![CD_SCOPE.into(), LS_SCOPE.into()],
            None,
            true,
        );
        prompt
    }

    fn type_text(prompt: &mut PermissionPrompt, text: &str) {
        let codes: Vec<KeyCode> = text.chars().map(KeyCode::Char).collect();
        press(prompt, &codes);
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn editing(prompt: &PermissionPrompt) -> bool {
        prompt
            .queue
            .front()
            .is_some_and(|request| request.rule_editor.is_some())
    }

    fn scope_marks(prompt: &PermissionPrompt) -> Vec<&'static str> {
        prompt
            .build_lines()
            .iter()
            .flat_map(|line| line.spans.iter())
            .filter_map(|span| {
                [MATCH_MARK, MISS_MARK]
                    .into_iter()
                    .find(|mark| span.content == *mark)
            })
            .collect()
    }

    /// `e` seeds one editable row per proposed rule with the cursor at the
    /// end, so typing extends the rule and the confirm writes what was typed.
    #[test_case('s' => matches Some(PermissionAnswer::AllowSession { .. }) ; "session")]
    #[test_case('a' => matches Some(PermissionAnswer::AllowAlwaysProject { .. }) ; "project")]
    #[test_case('A' => matches Some(PermissionAnswer::AllowAlwaysGlobal { .. }) ; "global")]
    fn edited_rules_are_committed(open: char) -> Option<PermissionAnswer> {
        let mut prompt = open_with(ToolKey::native("bash"), CD_SCOPE);
        press(&mut prompt, &[KeyCode::Char(open), KeyCode::Char('e')]);
        assert!(editing(&prompt));
        press(&mut prompt, &[KeyCode::Backspace]);
        type_text(&mut prompt, "/repo/*");
        press(&mut prompt, &[KeyCode::Enter]);
        assert!(!editing(&prompt));
        let answered = prompt
            .handle_key(key(KeyCode::Enter))
            .expect("an answer");
        assert_eq!(rules_of(Some(answered.answer.clone())), vec!["cd /repo/*"]);
        Some(answered.answer)
    }

    #[test]
    fn esc_reverts_edits_to_the_shape_rules() {
        let mut prompt = open_with(ToolKey::native("bash"), CD_SCOPE);
        press(&mut prompt, &[KeyCode::Char('s'), KeyCode::Char('e')]);
        type_text(&mut prompt, "junk");
        press(&mut prompt, &[KeyCode::Esc]);
        assert_eq!(state_of(&prompt), PromptState::ConfirmAllowSession);
        assert_eq!(
            rules_of(answer(&mut prompt, key(KeyCode::Enter))),
            vec!["cd *"]
        );
    }

    #[test]
    fn up_down_pick_the_row_and_clamp_at_the_ends() {
        let mut prompt = open_two_scopes();
        press(
            &mut prompt,
            &[
                KeyCode::Char('s'),
                KeyCode::Char('e'),
                KeyCode::Down,
                KeyCode::Down,
            ],
        );
        type_text(&mut prompt, "x");
        press(&mut prompt, &[KeyCode::Up, KeyCode::Up]);
        type_text(&mut prompt, "y");
        press(&mut prompt, &[KeyCode::Enter]);
        assert_eq!(
            rules_of(answer(&mut prompt, key(KeyCode::Enter))),
            vec!["cd *y", "ls *x"]
        );
    }

    #[test]
    fn emptied_rows_are_dropped_on_accept() {
        let mut prompt = open_two_scopes();
        press(&mut prompt, &[KeyCode::Char('s'), KeyCode::Char('e')]);
        assert!(prompt.handle_key(ctrl('a')).is_none());
        assert!(prompt.handle_key(ctrl('k')).is_none());
        press(&mut prompt, &[KeyCode::Enter]);
        assert_eq!(
            rules_of(answer(&mut prompt, key(KeyCode::Enter))),
            vec!["ls *"]
        );
    }

    #[test]
    fn cycling_the_shape_replaces_edited_rules() {
        let mut prompt = open_with(ToolKey::native("bash"), CD_SCOPE);
        press(&mut prompt, &[KeyCode::Char('s'), KeyCode::Char('e')]);
        type_text(&mut prompt, "x");
        press(&mut prompt, &[KeyCode::Enter, KeyCode::Right]);
        assert_eq!(
            rules_of(answer(&mut prompt, key(KeyCode::Enter))),
            vec![CD_SCOPE]
        );
    }

    #[test]
    fn cancelled_confirm_keeps_edited_rules() {
        let mut prompt = open_with(ToolKey::native("bash"), CD_SCOPE);
        press(&mut prompt, &[KeyCode::Char('s'), KeyCode::Char('e')]);
        type_text(&mut prompt, "x");
        press(&mut prompt, &[KeyCode::Enter, KeyCode::Esc]);
        assert_eq!(state_of(&prompt), PromptState::Normal);
        press(&mut prompt, &[KeyCode::Char('a')]);
        assert_eq!(
            rules_of(answer(&mut prompt, key(KeyCode::Enter))),
            vec!["cd *x"]
        );
    }

    #[test]
    fn preview_follows_editing_from_the_initial_prompt() {
        let mut prompt = open_two_scopes();
        press(&mut prompt, &[KeyCode::Char('e')]);
        assert_eq!(scope_marks(&prompt), vec![MATCH_MARK, MATCH_MARK]);
    }

    #[test]
    fn paste_lands_in_the_selected_rule() {
        let mut prompt = open_two_scopes();
        press(
            &mut prompt,
            &[KeyCode::Char('s'), KeyCode::Char('e'), KeyCode::Down],
        );
        assert!(prompt.handle_paste(" --color"));
        press(&mut prompt, &[KeyCode::Enter]);
        assert_eq!(
            rules_of(answer(&mut prompt, key(KeyCode::Enter))),
            vec!["cd *", "ls * --color"]
        );
    }

    /// The preview follows the rules as they are typed: a rule that no longer
    /// covers its scope flips that scope red before the edit is accepted.
    #[test]
    fn preview_marks_scopes_by_the_live_rules() {
        let mut prompt = open_two_scopes();
        assert!(scope_marks(&prompt).is_empty());
        press(&mut prompt, &[KeyCode::Char('s')]);
        assert_eq!(scope_marks(&prompt), vec![MATCH_MARK, MATCH_MARK]);
        press(&mut prompt, &[KeyCode::Char('e'), KeyCode::Backspace]);
        type_text(&mut prompt, "/other/*");
        assert_eq!(scope_marks(&prompt), vec![MISS_MARK, MATCH_MARK]);
        press(&mut prompt, &[KeyCode::Enter]);
        assert_eq!(scope_marks(&prompt), vec![MISS_MARK, MATCH_MARK]);
        press(&mut prompt, &[KeyCode::Esc]);
        assert!(scope_marks(&prompt).is_empty());
    }
}
