pub const BACKEND_PROMPT: &str = include_str!("../templates/realtime/backend_prompt.md");
pub const END_INSTRUCTIONS: &str = include_str!("../templates/realtime/realtime_end.md");
pub const START_INSTRUCTIONS: &str = include_str!("../templates/realtime/realtime_start.md");

const DEFAULT_USER_FIRST_NAME: &str = "there";
const USER_FIRST_NAME_PLACEHOLDER: &str = "{{ user_first_name }}";

pub fn default_realtime_backend_prompt() -> String {
    BACKEND_PROMPT
        .trim_end()
        .replace(USER_FIRST_NAME_PLACEHOLDER, &current_user_first_name())
}

fn current_user_first_name() -> String {
    [whoami::realname(), whoami::username()]
        .into_iter()
        .filter_map(|name| name.split_whitespace().next().map(str::to_string))
        .find(|name| !name.is_empty())
        .unwrap_or_else(|| DEFAULT_USER_FIRST_NAME.to_string())
}
