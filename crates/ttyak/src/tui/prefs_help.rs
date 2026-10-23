//! The help box under F6's list: what the selected item does, what its values mean and how to set it.
//! The sentences go through `tr` (and are listed by [`strings`]); the bullets, the alignment and
//! the colour of the keys are drawn here.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::keymap::{Action, Context, Keymap};
use super::prefs::{CATEGORIES, Field, KEYS_SETTINGS};
use super::state::Workspace;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelpLine {
    /// A sentence or two, wrapped at words.
    Text(String),
    /// `• name   meaning`: the names line up, a long meaning hangs under itself.
    Value { name: String, meaning: String },
    /// `keys   what they do`, the keys in the accent colour.
    Key { keys: String, does: String },
    Blank,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Help {
    /// The item's name, as on its row.
    pub title: String,
    pub lines: Vec<HelpLine>,
}

/// Below room for this many rows of text (plus the borders), or the whole help if it is
/// shorter, the box is not drawn.
pub const MIN_TEXT_ROWS: u16 = 3;

/// The most rows (borders included) the help box keeps from a long list.
const MAX_KEPT_ROWS: u16 = 14;

/// Blank cells between the columns of a value or key line.
const GAP: usize = 2;

/// The help of a field, with the user's current keys named in it.
pub fn help(field: Field, ws: &Workspace, keys: &Keymap) -> Help {
    build(field, ws, keys, &|s| super::i18n::tr(s).into_owned())
}

/// The help shown while the Keys filter has the focus.
pub fn filter_help(keys: &Keymap) -> Help {
    build_filter(keys, &|s| super::i18n::tr(s).into_owned())
}

fn build_filter(keys: &Keymap, tr: &dyn Fn(&'static str) -> String) -> Help {
    let mut b = Builder { tr, keys, lines: Vec::new() };
    b.text("Matches names, descriptions and keys, in any case.");
    let (keep, clear) = (b.key_of(Action::Open), b.key_of(Action::Close));
    b.key(keep, "keep the filter and go back to the list");
    b.key(clear, "clear the filter and go back to the list");
    Help { title: tr("filter"), lines: b.lines }
}

/// Every sentence the helps say, as `tr` takes them (for `ctl locale-template`).
pub fn strings() -> Vec<&'static str> {
    let seen = std::cell::RefCell::new(Vec::<&'static str>::new());
    let (ws, keys) = (Workspace::default(), Keymap::load("").0);
    let mut fields: Vec<Field> = CATEGORIES.iter().flat_map(|c| c.1.iter().copied()).chain(KEYS_SETTINGS.iter().copied()).collect();
    // A group with a prefix and one without; a binding rebound, so its default shows too.
    let prefixed = super::keymap::keys_tree(&keys).iter().position(|s| s.prefix.is_some()).unwrap_or(0);
    // A binding with a <leader> key, which says what the leader is.
    let leader_bound = super::keymap::ACTIONS.iter().position(|a| a.name == "jump_to").unwrap_or(0);
    fields.extend([Field::ScheduleHours, Field::GroupHeader(0), Field::GroupHeader(prefixed), Field::Binding(leader_bound)]);
    fields.extend((0..7).map(Field::ScheduleDay));
    let rebound = Keymap::load("[global]\nquit = \"ctrl+g\"\n").0;
    build(Field::Binding(0), &ws, &rebound, &|s| {
        seen.borrow_mut().push(s);
        s.to_string()
    });
    build_filter(&keys, &|s| {
        seen.borrow_mut().push(s);
        s.to_string()
    });
    for field in fields {
        build(field, &ws, &keys, &|s| {
            seen.borrow_mut().push(s);
            s.to_string()
        });
    }
    let mut seen = seen.into_inner();
    let mut unique = std::collections::HashSet::new();
    seen.retain(|s| unique.insert(*s));
    seen
}

/// Collects the lines of one help.
struct Builder<'a> {
    tr: &'a dyn Fn(&'static str) -> String,
    keys: &'a Keymap,
    lines: Vec<HelpLine>,
}

impl Builder<'_> {
    fn text(&mut self, s: &'static str) -> &mut Self {
        self.lines.push(HelpLine::Text((self.tr)(s)));
        self
    }

    fn blank(&mut self) -> &mut Self {
        self.lines.push(HelpLine::Blank);
        self
    }

    /// A value as the row shows it (not translated), and what it means.
    fn value(&mut self, name: &str, meaning: &'static str) -> &mut Self {
        self.lines.push(HelpLine::Value { name: name.to_string(), meaning: (self.tr)(meaning) });
        self
    }

    /// A fact about the item: its name is translated, its meaning is already text.
    fn fact(&mut self, name: &'static str, meaning: String) -> &mut Self {
        self.lines.push(HelpLine::Value { name: (self.tr)(name), meaning });
        self
    }

    fn key(&mut self, keys: String, does: &'static str) -> &mut Self {
        self.lines.push(HelpLine::Key { keys, does: (self.tr)(does) });
        self
    }

    /// The user's first key for an action in the preferences.
    fn key_of(&self, action: Action) -> String {
        self.keys.first_key(Context::Preferences, action).unwrap_or_else(|| "(unbound)".into())
    }

    /// Enter changes it: `Enter` and what it does.
    fn change(&mut self, does: &'static str) -> &mut Self {
        let keys = self.key_of(Action::Open);
        self.key(keys, does)
    }

    /// Enter, + and - change it: `Enter / + -` and what they do.
    fn step(&mut self, does: &'static str) -> &mut Self {
        let keys = format!("{} / {} {}", self.key_of(Action::Open), self.key_of(Action::Increase), self.key_of(Action::Decrease));
        self.key(keys, does)
    }

    /// The three levels a notification setting goes through.
    fn levels(&mut self) -> &mut Self {
        self.value("all new messages", "every message notifies you").value("mentions and DMs", "only messages that mention you, and direct messages").value("nothing", "no message notifies you")
    }
}

fn build(field: Field, _ws: &Workspace, keys: &Keymap, tr: &dyn Fn(&'static str) -> String) -> Help {
    let mut b = Builder { tr, keys, lines: Vec::new() };
    // The title of a row drawn from the keys tree (its own label), else the field's.
    let mut title = None;
    match field {
        // Availability
        Field::Status => {
            b.text("Your status on Slack: an emoji and a short text people see next to your name.").blank();
            b.change("edit it, Enter again saves; empty clears the status");
            b.key(":emoji: text".into(), "the form to type; add \" for 30m\" (or 2h, 1d, today) to make it clear itself");
        }
        Field::Away => {
            b.text("Shows you as away on Slack, whatever you are doing, until you switch it off. Kept on Slack.");
        }
        Field::Notifications => {
            b.text("Pauses the notifications Slack sends you, or lets them through again. Kept on Slack.").blank();
            b.change("pause for 30 minutes, or resume");
            let keys = format!("{} {}", b.key_of(Action::Increase), b.key_of(Action::Decrease));
            b.key(keys, "lengthen or shorten a running pause: 30 min, 1 h, 2 h, 8 h");
        }
        Field::Schedule => {
            b.text("The hours you receive notifications; outside them they are paused. Changes are saved to Slack at once.").blank();
            b.value("off", "notifications come at any time").value("every day", "the same hours every day").value("weekdays", "the hours on Monday to Friday: paused from each of those evenings to the next morning, so from Saturday morning to Monday evening notifications come at any time").value("custom", "hours of its own for each day, listed below").blank();
            b.step("change the kind");
        }
        Field::ScheduleHours => {
            b.text("The hours you receive notifications; outside them they are paused. Saved to Slack at once.").blank();
            b.change("type the hours as 9:00-17:00 (24-hour), Enter again saves");
        }
        Field::ScheduleDay(_) => {
            b.text("Notifications on this day of the custom schedule.").blank();
            b.value("9:00-17:00", "they come through between these hours").value("receive all day", "no quiet hours").value("paused all day", "nothing comes through").blank();
            b.change("edit the hours, or type receive all day / paused all day");
            let keys = format!("{} {}", b.key_of(Action::Increase), b.key_of(Action::Decrease));
            b.key(keys, "go through hours, receive all day, paused all day (hours again: the every-day ones)");
        }
        // Appearance
        Field::Theme => {
            b.text("The colours ttyak uses: a built-in theme, the terminal's own colours, or a theme file of yours.").blank();
            b.change("choose from a list; each theme shows as you move, Esc goes back");
        }
        Field::WorkspaceTheme => {
            b.text("A colour theme for this workspace alone, to tell workspaces apart. Without one, the colour theme is used.").blank();
            b.change("choose from a list; each theme shows as you move, Esc goes back");
        }
        Field::Compact => {
            b.text("No blank rows between authors in messages, or between sections of the sidebar.");
        }
        Field::HideInactive => {
            b.text("Channels and DMs without a message for this long leave the sidebar. Starred, unread and open ones stay, and the switcher still finds the rest.").blank();
            b.step("never, 1, 7, 14, 30, 60 or 90 days");
        }
        // Messages & media
        Field::InlineImages => {
            b.text("Shows attached images as thumbnails in the message list, where the terminal draws pictures.");
        }
        Field::Avatars => {
            b.text("Shows profile pictures next to author names, where the terminal draws pictures.");
        }
        Field::LargeAvatars => {
            b.text("Makes profile pictures two rows tall, beside the name and the first line of the text.");
        }
        Field::PreviewRefresh => {
            b.text("A channel you only preview (you haven't joined it) gets no live events, so it is fetched again this often.").blank();
            b.step("off, or every 5, 10, 15, 30, 60 or 120 seconds");
        }
        Field::StatusRefresh => {
            b.text("Slack pushes people's status changes to ttyak as they happen, as to its own client, and ttyak then looks that one person up. As a safety net — and for the rare connection without pushes (when Slack offers no gateway or it fails twice in a row, ttyak falls back to rtm.connect for a while) — the people shown, the direct messages and group DMs in the sidebar and the authors in the open conversation and thread, are also looked up again this often, and once after the connection was lost. An expired status goes at once either way.").blank();
            b.step("off, or every 60, 300, 600 (the default), 1800 or 3600 seconds");
        }
        Field::ThreadRead => {
            b.text("In the Threads view (Ctrl+T), a thread is marked read on Slack once its whole block — or, for one taller than the view, its unread replies — has been on screen this long, not while you scroll past. Opening it (Enter) or replying in it marks it read as well.").blank();
            b.step("only when opened, or after 1, 2, 3, 5 or 10 seconds");
        }
        Field::Graphics => {
            graphics_values(&mut b, super::media::drawable());
            b.change("the next way");
        }
        Field::EmojiAsNames => {
            b.text("Shows emoji as :names: instead of the characters, for terminals that draw them badly.");
        }
        Field::CustomEmojiPictures => {
            b.text("Draws your workspace's own picture emoji as pictures in messages and reactions, where the terminal draws pictures; otherwise they show as :name:.");
        }
        Field::SaveDir => {
            b.text("The folder a file is offered to be saved to first (~ is your home). Empty: your downloads folder. TTYAK_DOWNLOAD_DIR, when set, wins over this.").blank();
            b.change("type a folder, Enter again keeps it");
        }
        // Language & region
        Field::Clock24 => {
            b.text("Shows times as 13:05 when on, as 1:05 pm when off.");
        }
        Field::DateFormat => {
            b.text("How the date shows in the separators between days of messages.").blank();
            b.change("the next format");
        }
        Field::TimeZone => {
            b.text("The time zone ttyak shows times in. Empty: the system's zone.").blank();
            b.change("type a zone such as Europe/Kyiv, Enter again keeps it");
        }
        Field::Language => {
            b.text("The language of ttyak's own texts. English is built in; others are read from locales/<code>.toml in ttyak's config folder, and ttyak ctl locale-template writes an empty one to translate.").blank();
            b.change("type the code (such as de), Enter again applies it");
        }
        // Notifications
        Field::Desktop => {
            b.text("Shows a desktop notification for a message that notifies you, as the levels below decide.");
        }
        Field::Sound => {
            b.text("The sound played with each notification. How loud is Notification sound volume under Audio & video.").blank();
            b.value("off", "silent").value("chime, ping, knock", "built-in sounds").value("system", "the desktop's own notification sound").blank();
            b.change("the next sound, played for you to hear");
        }
        Field::ConversationLevel => {
            b.text("What notifies you in the conversation on screen. Kept on Slack; it needs a conversation open.").blank();
            b.levels().blank();
            b.change("the next level; after nothing, back to the channels' default");
        }
        Field::ChannelDefault => {
            b.text("What notifies you in channels that have no setting of their own. Kept on Slack.").blank();
            b.levels().blank();
            b.change("the next level");
        }
        Field::Keywords => {
            b.text("Words that notify you like a mention. Kept on Slack.").blank();
            b.change("type them separated by commas, Enter again saves");
        }
        Field::Vips => {
            b.text("The people you marked as VIPs, whose messages Slack notifies you about. Kept on Slack. You add or remove one on their profile card.");
        }
        // Audio & video
        Field::Input => {
            b.text("The microphone used in huddles. TTYAK_AUDIO_INPUT, when set, wins over this.").blank();
            b.change("the next one: the system default, then each microphone found");
        }
        Field::Output => {
            b.text("The speaker used in huddles and for sounds. TTYAK_AUDIO_OUTPUT, when set, wins over this.").blank();
            b.change("the next one: the system default, then each speaker found");
        }
        Field::HuddleVolume => {
            b.text("How loud the others sound in a huddle; a huddle you are in follows at once.").blank();
            b.step("change it in steps, 0 to 200 %");
        }
        Field::SoundVolume => {
            b.text("How loud notification and huddle sounds play; you hear the new level as you set it.").blank();
            b.step("change it in steps, 0 to 200 %");
        }
        Field::MicGain => {
            b.text("How much your microphone is amplified in a huddle; it follows at once.").blank();
            b.step("change it in steps, 25 to 400 %");
        }
        Field::SpeakingThreshold => {
            b.text("Below this level the microphone sends silence, so quiet background noise isn't heard. Off: the microphone always sends.").blank();
            b.step("off, or -60 to -20 dB in steps of 5 (higher is stricter)");
        }
        Field::NoiseSuppression => {
            b.text("Filters steady background noise out of your microphone in a huddle.");
        }
        Field::EchoCancellation => {
            b.text("Removes what your speaker plays from your microphone, so the others don't hear themselves.");
        }
        Field::Camera => {
            b.text("The camera a huddle can send. None keeps it off; ttyak never picks one for you, and nothing is opened here.").blank();
            b.change("none, then each camera found");
        }
        // Huddles
        Field::JoinSound => {
            b.text("Played when you join a huddle, once it is connected.").blank();
            b.change("the next sound, played for you to hear");
        }
        Field::SomeoneJoined => {
            b.text("Played when someone else joins the huddle you are in.").blank();
            b.change("the next sound, played for you to hear");
        }
        Field::SomeoneLeft => {
            b.text("Played when someone leaves the huddle you are in.").blank();
            b.change("the next sound, played for you to hear");
        }
        Field::RingSound => {
            b.text("Played every few seconds while a huddle invitation rings, until you answer or it stops ringing.").blank();
            b.change("the next sound, played for you to hear");
        }
        Field::RingTimeout => {
            b.text("How long an invitation rings before it stops.").blank();
            b.step("15, 30, 45, 60, 90 or 120 seconds");
        }
        Field::AutoDecline => {
            b.text("When an invitation rings out unanswered, also declines it on Slack, so the caller sees it was.");
        }
        Field::AloneSound => {
            b.text("A quiet melody that loops when you have been the only one in a huddle for the time below, until someone joins or you leave. It plays softer than the other sounds.").blank();
            b.change("the next one, played for you to hear");
        }
        Field::AloneSecs => {
            b.text("How long you may be alone in a huddle before the reminder starts.").blank();
            b.step("30 s, 1, 2, 3, 5, 10 or 15 minutes");
        }
        // Updates
        Field::UpdateVersion => {
            b.text("This ttyak's version and how it was installed: with the install script, by a package manager (Homebrew, Scoop, winget, Chocolatey, the AUR, a .deb or an .rpm package), or unknown — a copy downloaded by hand. How it was installed decides how it is updated.");
        }
        Field::UpdateCheck => {
            b.text("Looks for a new version of ttyak now and then: a plain request for the site's latest.json, which sends nothing about you but ttyak's name and version. A new version is told once: a desktop notification (when they are on) and ⬆ in the status line. Off: no check at all, unless you ask for one (Check now).");
        }
        Field::UpdateEvery => {
            b.text("How often the check runs at most, in seconds; a check that got no answer counts too. Less than an hour (written by hand in settings.toml) counts as an hour.").blank();
            b.step("1, 6 or 12 hours, or 1, 3 or 7 days");
        }
        Field::UpdateTimeout => {
            b.text("The update screen's download gives up when no data has come for this long; the old ttyak stays as it was. Written by hand in settings.toml, it counts as at least 10 seconds and at most a day.").blank();
            b.step("10, 15, 30, 60, 120 or 300 seconds");
        }
        Field::UpdateCheckNow => {
            b.text("Looks for a new version at once, whether a check is due or not, with the check off too. It never shows a desktop notification: what it finds is said here, and a new version adds the Update row below. A version found this way is not announced later either (no notification, no ⬆ in the status line), but whenever a check finds it again — the daily one in a later run too — the Update row is back here.").blank();
            b.change("check now (a press while one runs does nothing)");
        }
        Field::UpdateCommand => {
            b.text("How to update ttyak for the way it was installed: the package manager's command, the install script's for a folder ttyak cannot write, or the page to download a new .deb, .rpm or archive from. A copy the install script put in a folder ttyak can write updates itself: Update, once a new version is out.").blank();
            b.change("copy the command or the link");
        }
        Field::UpdateOpen => {
            b.text("A newer ttyak is out: the update screen updates this one, or says what to run.").blank();
            b.change("open the update screen");
        }
        // Keys, above the bindings
        Field::WhichKeyShow => {
            b.text("The panel that lists the keys that can come next while you press a sequence of keys.");
        }
        Field::WhichKeyCorner => {
            b.text("The corner of the screen the which-key panel sits in.").blank();
            b.change("the next corner: bottom-right, bottom-left, top-left, top-right");
        }
        Field::WhichKeyDelay => {
            b.text("How long after the last key the which-key panel opens; 0 opens it at once.").blank();
            b.step("change it in steps of 0.1 s, up to 5 s");
        }
        Field::WhichKeyHeight => {
            b.text("The tallest the which-key panel gets, in percent of the screen's height; a longer list pages.").blank();
            b.step("change it in steps of 10 %, up to 100 %");
        }
        Field::WhichKeyIcons => {
            b.text("Shows an icon before each label in which-key, in the help and in this Keys list.");
        }
        Field::Leader => {
            b.text("The key that starts the sequences written <leader> in the bindings, such as <leader> k.").blank();
            leader_keys(&mut b);
        }
        Field::InsertLeader => {
            b.text("The <leader> while typing a message and in dialogs, where plain keys type text.").blank();
            leader_keys(&mut b);
        }
        Field::GroupHeader(n) => {
            let section = super::keymap::keys_tree(keys).into_iter().nth(n);
            if section.as_ref().is_some_and(|s| s.prefix.is_some()) {
                b.text("Keys that start the same way.");
                let (rename, icon) = (b.key_of(Action::Open), b.key_of(Action::EditIcon));
                b.key(rename, "rename it, Enter again saves (empty: the default)");
                b.key(icon, "change its icon, Enter again saves (empty: the default)");
            } else {
                b.text("Keys gathered by where they work. This heading has no name or icon to change.");
            }
            title = section.map(|s| s.title);
        }
        Field::Binding(i) => {
            let info = &super::keymap::ACTIONS[i];
            let unbound = tr("(unbound)");
            b.text(info.description).blank();
            let works = if info.context == Context::Global { format!("{} (Global)", tr("everywhere")) } else { tr(info.context.title()) };
            b.fact("Works", works);
            let now = keys.keys_named(info.context, info.name);
            b.fact("Keys", if now.is_empty() { unbound.clone() } else { now.clone() });
            // What <leader> is pressed as here (the insert leader while typing and in dialogs).
            if let Some(leader) = keys.leader_in(info.context).filter(|_| now.contains("<leader>")) {
                b.fact("<leader>", leader.label());
            }
            let default = keys.default_named(info.context, info.name);
            if default != now {
                b.fact("Default", if default.is_empty() { unbound } else { default });
            }
            b.lines.push(HelpLine::Value { name: "keys.toml".into(), meaning: info.name.into() });
            b.blank();
            let (open, unbind, reset) = (b.key_of(Action::Open), b.key_of(Action::Unbind), b.key_of(Action::Reset));
            b.key(open, "record new keys (until a 1 s pause; any key counts)");
            b.key(unbind, "remove its keys");
            b.key(reset, "back to the default keys");
            title = super::keymap::keys_tree(keys).into_iter().flat_map(|s| s.rows).find(|r| r.info == i).map(|r| r.label);
        }
    }
    Help { title: title.unwrap_or_else(|| field.label()), lines: b.lines }
}

/// How to record, remove or reset a leader.
/// The help of "Draw pictures with": every way, the ones this terminal cannot draw (not in
/// `drawable`) marked, and how to force one.
fn graphics_values(b: &mut Builder, drawable: &[&str]) {
    b.text("How pictures are drawn in the terminal. Only the ways this terminal said it can draw are offered. To force another (over ssh or tmux the terminal may not answer), start ttyak with TTYAK_GRAPHICS set to its name: it wins over this.").blank();
    let not_here = (b.tr)("not available in this terminal");
    let ways = [("auto", "ask the terminal what it can draw"), ("kitty", "the kitty graphics protocol"), ("sixel", "sixel graphics"), ("iterm2", "the iTerm2 image protocol"), ("halfblocks", "coloured blocks: works everywhere, but no profile pictures")];
    for (name, meaning) in ways {
        b.value(name, meaning);
        if !drawable.contains(&name) {
            if let Some(HelpLine::Value { meaning, .. }) = b.lines.last_mut() {
                meaning.push_str(&format!(" — {not_here}"));
            }
        }
    }
    b.blank();
}

fn leader_keys(b: &mut Builder) {
    let (open, unbind, reset) = (b.key_of(Action::Open), b.key_of(Action::Unbind), b.key_of(Action::Reset));
    b.key(open, "record the new key (until a 1 s pause; any key counts)");
    b.key(unbind, "have none");
    b.key(reset, "back to the default");
}

/// The styled lines of a help, wrapped to `width` cells.
pub fn layout(help: &Help, width: u16) -> Vec<Line<'static>> {
    let width = width.max(1) as usize;
    let accent = Style::default().fg(super::theme::p().accent);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let name_w = help.lines.iter().filter_map(|l| if let HelpLine::Value { name, .. } = l { Some(name.width()) } else { None }).max().unwrap_or(0);
    let keys_w = help.lines.iter().filter_map(|l| if let HelpLine::Key { keys, .. } = l { Some(keys.width()) } else { None }).max().unwrap_or(0);
    let mut out = Vec::new();
    for line in &help.lines {
        match line {
            HelpLine::Blank => out.push(Line::raw("")),
            HelpLine::Text(t) => out.extend(wrap(t, width).into_iter().map(Line::raw)),
            HelpLine::Value { name, meaning } => {
                let column = 2 + name_w + GAP;
                let mut rows = wrap(meaning, width.saturating_sub(column).max(1)).into_iter();
                out.push(Line::from(vec![Span::raw("• "), Span::styled(name.clone(), bold), Span::raw(" ".repeat(column - 2 - name.width())), Span::raw(rows.next().unwrap_or_default())]));
                out.extend(rows.map(|r| Line::raw(format!("{}{r}", " ".repeat(column)))));
            }
            HelpLine::Key { keys, does } => {
                let column = keys_w + GAP + 1;
                let mut rows = wrap(does, width.saturating_sub(column).max(1)).into_iter();
                out.push(Line::from(vec![Span::styled(keys.clone(), accent), Span::raw(" ".repeat(column - keys.width())), Span::raw(rows.next().unwrap_or_default())]));
                out.extend(rows.map(|r| Line::raw(format!("{}{r}", " ".repeat(column)))));
            }
        }
    }
    out
}

/// `text` broken at spaces into rows of at most `width` cells (a longer word is cut).
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut rows: Vec<String> = Vec::new();
    let mut row = String::new();
    for word in text.split_whitespace() {
        let mut word = word.to_string();
        while word.width() > width {
            // Cut a word longer than a row where the row ends.
            if !row.is_empty() {
                rows.push(std::mem::take(&mut row));
            }
            let mut used = 0;
            let cut = word.char_indices().find(|(_, c)| { used += c.width().unwrap_or(0); used > width }).map_or(word.len(), |(i, _)| i.max(word.chars().next().map_or(0, char::len_utf8)));
            rows.push(word[..cut].to_string());
            word = word[cut..].to_string();
        }
        if !row.is_empty() && row.width() + 1 + word.width() > width {
            rows.push(std::mem::take(&mut row));
        }
        if !row.is_empty() {
            row.push(' ');
        }
        row.push_str(&word);
    }
    if !row.is_empty() || rows.is_empty() {
        rows.push(row);
    }
    rows
}

/// The rows of a box `outer` cells wide and `room` rows high, or None when it does not fit:
/// room for [`MIN_TEXT_ROWS`] rows of text and the borders (less if the help is shorter) is the least.
pub fn box_height(help: &Help, outer: u16, room: u16) -> Option<u16> {
    let rows = layout(help, outer.saturating_sub(2)).len() as u16;
    (room >= rows.min(MIN_TEXT_ROWS) + 2).then(|| (rows + 2).min(room))
}

/// How many rows of `height` a list longer than that shows: the help box under it keeps two
/// fifths (at least its least, at most [`MAX_KEPT_ROWS`]) and a blank row, the same for every
/// item so the list stays still as the selection moves.
pub fn list_rows(height: u16) -> u16 {
    let kept = (height * 2 / 5).clamp(MIN_TEXT_ROWS + 2, MAX_KEPT_ROWS);
    height.saturating_sub(kept + 1).max(1)
}

/// `lines` cut to `rows`; the last of a cut help ends in "…".
fn fitted(mut lines: Vec<Line<'static>>, rows: usize, width: usize) -> Vec<Line<'static>> {
    if lines.len() <= rows {
        return lines;
    }
    lines.truncate(rows);
    let Some(last) = lines.last_mut() else { return lines };
    // Make room for the ellipsis at the end of the row.
    while last.width() + 1 > width {
        let Some(span) = last.spans.last_mut() else { break };
        let mut text = span.content.to_string();
        if text.pop().is_none() {
            last.spans.pop();
        } else {
            span.content = text.into();
        }
    }
    last.spans.push(Span::raw("…"));
    lines
}

/// Draws the help in a rounded, dim box filling `area`.
pub fn draw(f: &mut Frame, area: Rect, help: &Help) {
    let dim = Style::default().fg(super::theme::p().dim);
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(dim).title(Line::styled(format!(" {} ", help.title), dim.add_modifier(Modifier::BOLD)));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let lines = fitted(layout(help, inner.width), inner.height as usize, inner.width as usize);
    f.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::keymap::Keymap;
    use crate::tui::prefs::{CATEGORIES, Field};

    fn all_fields() -> Vec<Field> {
        let mut all: Vec<Field> = CATEGORIES.iter().flat_map(|c| c.1.iter().copied()).collect();
        all.push(Field::ScheduleHours);
        all.extend((0..7).map(Field::ScheduleDay));
        all.extend(crate::tui::prefs::KEYS_SETTINGS.iter().copied());
        all
    }

    fn keys() -> Keymap {
        Keymap::load("").0
    }

    fn texts(h: &Help) -> usize {
        h.lines.iter().filter(|l| matches!(l, HelpLine::Text(_))).count()
    }

    fn values(h: &Help) -> Vec<&str> {
        h.lines.iter().filter_map(|l| if let HelpLine::Value { name, .. } = l { Some(name.as_str()) } else { None }).collect()
    }

    fn key_lines(h: &Help) -> Vec<&str> {
        h.lines.iter().filter_map(|l| if let HelpLine::Key { keys, .. } = l { Some(keys.as_str()) } else { None }).collect()
    }

    fn plain(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn lines_of(n: usize) -> Help {
        Help { title: "t".into(), lines: (0..n).map(|i| HelpLine::Text(format!("line {i}"))).collect() }
    }

    #[test]
    fn every_item_has_help() {
        let ws = Workspace::default();
        for field in all_fields() {
            let h = help(field, &ws, &keys());
            assert!(!h.title.is_empty(), "{field:?} has a title");
            assert_eq!(h.title, field.label(), "{field:?}: the title is the row's label");
            assert!(texts(&h) >= 1, "{field:?} says what it does");
        }
    }

    #[test]
    fn graphics_marks_what_this_terminal_cannot_draw() {
        let map = keys();
        let tr = |s: &'static str| s.to_string();
        let mut b = Builder { tr: &tr, keys: &map, lines: Vec::new() };
        graphics_values(&mut b, &["auto", "kitty", "halfblocks"]);
        let h = Help { title: "t".into(), lines: b.lines };
        assert_eq!(values(&h), ["auto", "kitty", "sixel", "iterm2", "halfblocks"], "every way is listed");
        assert_eq!(value_of(&h, "kitty"), Some("the kitty graphics protocol"));
        assert_eq!(value_of(&h, "sixel"), Some("sixel graphics — not available in this terminal"));
        assert_eq!(value_of(&h, "iterm2"), Some("the iTerm2 image protocol — not available in this terminal"));
        assert!(!value_of(&h, "halfblocks").unwrap().contains("not available"));
        let HelpLine::Text(first) = &h.lines[0] else { panic!("{h:?}") };
        assert!(first.contains("TTYAK_GRAPHICS"), "{first}");
        // Before detection (and in tests) every way counts as drawable.
        let h = help(Field::Graphics, &Workspace::default(), &map);
        assert!(h.lines.iter().all(|l| !matches!(l, HelpLine::Value { meaning, .. } if meaning.contains("not available"))));
        assert!(strings().contains(&"not available in this terminal"), "translatable");
    }

    #[test]
    fn the_schedule_lists_its_kinds() {
        let h = help(Field::Schedule, &Workspace::default(), &keys());
        assert_eq!(values(&h), ["off", "every day", "weekdays", "custom"]);
        assert!(key_lines(&h).iter().any(|k| k.contains("Enter")), "{:?}", key_lines(&h));
        // What the weekend does with "weekdays" (`Schedule::quiet_at`): Friday's pause runs into Saturday morning, then free.
        let weekdays = value_of(&h, "weekdays").unwrap();
        assert!(weekdays.contains("from Saturday morning to Monday evening notifications come at any time"), "{weekdays}");
    }

    #[test]
    fn a_custom_day_explains_its_values() {
        let h = help(Field::ScheduleDay(0), &Workspace::default(), &keys());
        assert!(values(&h).contains(&"receive all day") && values(&h).contains(&"paused all day"));
        let all: String = h.lines.iter().map(|l| match l { HelpLine::Text(t) => t.clone(), HelpLine::Value { name, meaning } => format!("{name} {meaning}"), HelpLine::Key { keys, does } => format!("{keys} {does}"), HelpLine::Blank => String::new() }).collect();
        assert!(all.contains("9:00-17:00"));
    }

    #[test]
    fn on_off_items_are_short() {
        let h = help(Field::Compact, &Workspace::default(), &keys());
        assert!(key_lines(&h).is_empty());
        assert!(texts(&h) <= 2, "{h:?}");
    }

    #[test]
    fn help_names_your_keys() {
        let (rebound, warnings) = Keymap::load("[preferences]\nchange = \"x\"\n");
        assert!(warnings.is_empty(), "{warnings:?}");
        let h = help(Field::Schedule, &Workspace::default(), &rebound);
        let keys = key_lines(&h);
        assert!(keys.iter().any(|k| k.starts_with('x')), "{keys:?}");
        assert!(keys.iter().all(|k| !k.contains("Enter")), "{keys:?}");
    }

    #[test]
    fn the_box_disappears_below_its_minimum() {
        let h = lines_of(6);
        assert_eq!(box_height(&h, 60, 4), None, "3 text rows + 2 borders = 5 needed");
        assert_eq!(box_height(&h, 60, 5), Some(5));
        assert_eq!(box_height(&h, 60, 20), Some(8));
    }

    #[test]
    fn a_short_help_needs_less() {
        assert_eq!(box_height(&lines_of(2), 60, 4), Some(4));
    }

    #[test]
    fn a_long_list_leaves_the_help_room() {
        assert_eq!(list_rows(12), 6, "the least box: 3 rows of text");
        assert_eq!(list_rows(30), 17, "two fifths");
        assert_eq!(list_rows(60), 45, "at most 14 rows");
        assert_eq!(list_rows(3), 1);
    }

    #[test]
    fn values_align_and_wrap() {
        let long = "this meaning is long enough that it has to go on to a second and a third row of the box";
        let h = Help { title: "t".into(), lines: vec![HelpLine::Value { name: "off".into(), meaning: long.into() }, HelpLine::Value { name: "every day".into(), meaning: "x".into() }] };
        let lines = layout(&h, 40);
        assert!(lines.len() >= 3, "wrapped: {lines:?}");
        let column = |line: &Line, text: &str| plain(line).find(text).map(|i| plain(line)[..i].chars().count());
        let first = column(&lines[0], "this").unwrap();
        let last = lines.iter().find(|l| plain(l).contains("every day")).unwrap();
        assert_eq!(column(last, "x"), Some(first), "meanings start in one column");
        assert_eq!(plain(&lines[1]).chars().take(first).collect::<String>().trim(), "", "the continuation hangs under the meaning");
        assert!(lines[1].spans.iter().all(|s| !s.style.add_modifier.contains(Modifier::BOLD)));
        assert!(lines[0].spans.iter().any(|s| s.content == "off" && s.style.add_modifier.contains(Modifier::BOLD)), "names are bold");
        assert!(lines.iter().all(|l| plain(l).chars().count() <= 40));
    }

    #[test]
    fn a_cut_help_ends_in_an_ellipsis() {
        let lines = fitted(layout(&lines_of(6), 40), 4, 40);
        assert_eq!(lines.len(), 4);
        assert!(plain(&lines[3]).ends_with('…'), "{:?}", plain(&lines[3]));
        assert_eq!(fitted(layout(&lines_of(3), 40), 4, 40).len(), 3, "a help that fits is untouched");
    }

    #[test]
    fn help_is_translatable() {
        let first = match &help(Field::Schedule, &Workspace::default(), &keys()).lines[0] {
            HelpLine::Text(t) => t.clone(),
            other => panic!("{other:?}"),
        };
        assert!(strings().iter().any(|s| *s == first), "{first}");
        assert!(crate::tui::prefs::translatable().contains(&first));
    }

    fn jump_to() -> usize {
        crate::tui::keymap::ACTIONS.iter().position(|a| a.context == Context::Global && a.name == "jump_to").unwrap()
    }

    fn value_of<'a>(h: &'a Help, name: &str) -> Option<&'a str> {
        h.lines.iter().find_map(|l| match l { HelpLine::Value { name: n, meaning } if n == name => Some(meaning.as_str()), _ => None })
    }

    fn key_does<'a>(h: &'a Help, keys: &str) -> Option<&'a str> {
        h.lines.iter().find_map(|l| match l { HelpLine::Key { keys: k, does } if k == keys => Some(does.as_str()), _ => None })
    }

    #[test]
    fn a_binding_says_where_it_works_and_its_keys() {
        let map = keys();
        let i = jump_to();
        let h = help(Field::Binding(i), &Workspace::default(), &map);
        let info = &crate::tui::keymap::ACTIONS[i];
        assert!(h.lines.contains(&HelpLine::Text(info.description.to_string())), "{h:?}");
        assert_eq!(value_of(&h, "Works"), Some("everywhere (Global)"));
        assert_eq!(value_of(&h, "Keys"), Some(map.keys_named(Context::Global, "jump_to").as_str()));
        assert_eq!(value_of(&h, "<leader>"), Some("Space"));
        assert_eq!(value_of(&h, "keys.toml"), Some("jump_to"));
        assert_eq!(key_lines(&h), ["Enter", "Delete", "r"]);
        let other = crate::tui::keymap::ACTIONS.iter().position(|a| a.context == Context::Sidebar).unwrap();
        let h = help(Field::Binding(other), &Workspace::default(), &map);
        assert_eq!(value_of(&h, "Works"), Some(Context::Sidebar.title()));
    }

    #[test]
    fn the_default_shows_only_when_changed() {
        let i = jump_to();
        let h = help(Field::Binding(i), &Workspace::default(), &keys());
        assert_eq!(value_of(&h, "Default"), None);
        let (rebound, warnings) = Keymap::load("[global]\njump_to = \"ctrl+g\"\n");
        assert!(warnings.is_empty(), "{warnings:?}");
        let h = help(Field::Binding(i), &Workspace::default(), &rebound);
        assert_eq!(value_of(&h, "Keys"), Some("Ctrl+G"));
        assert_eq!(value_of(&h, "Default"), Some(keys().keys_named(Context::Global, "jump_to").as_str()));
        assert_eq!(value_of(&h, "<leader>"), None, "no <leader> in Ctrl+G");
    }

    #[test]
    fn a_group_row_says_how_to_rename() {
        let map = keys();
        let n = crate::tui::keymap::keys_tree(&map).iter().position(|s| s.prefix.is_some()).unwrap();
        let h = help(Field::GroupHeader(n), &Workspace::default(), &map);
        assert_eq!(key_lines(&h), ["Enter", "i"]);
        assert!(key_does(&h, "Enter").unwrap().contains("rename") && key_does(&h, "i").unwrap().contains("icon"), "{h:?}");
        assert!(texts(&h) >= 1);
    }

    #[test]
    fn the_filter_explains_itself() {
        let h = filter_help(&keys());
        assert_eq!(key_lines(&h), ["Enter", "Esc"]);
        assert!(key_does(&h, "Enter").unwrap().contains("keep") && key_does(&h, "Esc").unwrap().contains("clear"), "{h:?}");
        assert!(texts(&h) >= 1);
        assert!(strings().iter().any(|s| matches!(&h.lines[0], HelpLine::Text(t) if t == *s)), "listed for translation");
    }

    #[test]
    fn only_a_prefixed_group_can_be_renamed() {
        let map = keys();
        let tree = crate::tui::keymap::keys_tree(&map);
        let plain = tree.iter().position(|s| s.prefix.is_none()).unwrap();
        let prefixed = tree.iter().position(|s| s.prefix.is_some()).unwrap();
        let h = help(Field::GroupHeader(plain), &Workspace::default(), &map);
        assert!(key_lines(&h).is_empty() && texts(&h) >= 1, "{h:?}");
        assert_eq!(h.title, tree[plain].title);
        assert_eq!(key_lines(&help(Field::GroupHeader(prefixed), &Workspace::default(), &map)), ["Enter", "i"]);
    }

    #[test]
    fn a_changed_binding_shows_its_default() {
        let (rebound, _) = Keymap::load("[global]\nquit = \"ctrl+g\"\n");
        let h = help(Field::Binding(0), &Workspace::default(), &rebound);
        assert_eq!(value_of(&h, "Default"), Some("Ctrl+Q"));
    }

    #[test]
    fn every_binding_and_group_sentence_is_listed() {
        let listed = strings();
        for want in ["Works", "Keys", "Default", "everywhere", "(unbound)", "Keys that start the same way.", "Keys gathered by where they work. This heading has no name or icon to change.", "record new keys (until a 1 s pause; any key counts)"] {
            assert!(listed.contains(&want), "{want}");
        }
    }
}
