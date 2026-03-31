//! Command arity dictionary for bash permission patterns.
//!
//! When a user clicks "Always Allow" on a bash command, we don't want to store
//! the exact command string. Instead, we extract a canonical prefix that captures
//! the "kind" of command, so that approving `git commit -m 'fix'` also auto-approves
//! `git commit -m 'refactor'`.
//!
//! The arity dictionary maps command prefixes to the number of tokens that form
//! the "command identity". For example:
//!
//! - `git` has arity 2: `git commit`, `git push`, `git status` are distinct commands
//! - `aws` has arity 3: `aws s3 cp`, `aws ec2 describe-instances` are distinct
//! - `ls` has arity 1: all `ls` invocations are the same kind
//!
//! Ported from `OpenCode`'s `permission/arity.ts`.

use std::collections::HashMap;
use std::sync::LazyLock;

/// Given command tokens, return the canonical prefix based on the arity dictionary.
///
/// The prefix represents the "command identity" — the minimum number of tokens
/// needed to distinguish this command from others of the same tool.
///
/// # Examples
///
/// ```
/// use flok_core::permission::arity::command_prefix;
///
/// assert_eq!(command_prefix(&["git", "commit", "-m", "fix"]), vec!["git", "commit"]);
/// assert_eq!(command_prefix(&["npm", "install", "express"]), vec!["npm", "install"]);
/// assert_eq!(command_prefix(&["aws", "s3", "cp", "file", "s3://bucket"]), vec!["aws", "s3", "cp"]);
/// assert_eq!(command_prefix(&["ls", "-la"]), vec!["ls"]);
/// assert_eq!(command_prefix(&["unknown-tool", "--flag"]), vec!["unknown-tool"]);
/// assert!(command_prefix(&[]).is_empty());
/// ```
pub fn command_prefix<'a>(tokens: &[&'a str]) -> Vec<&'a str> {
    // Try longest-prefix-first matching against the arity dictionary.
    for len in (1..=tokens.len()).rev() {
        let prefix: String = tokens[..len].join(" ");
        if let Some(&arity) = ARITY.get(prefix.as_str()) {
            // Return the first `arity` tokens (or all if arity > len)
            let take = arity.min(tokens.len());
            return tokens[..take].to_vec();
        }
    }

    // No match — default to the first token
    if tokens.is_empty() {
        vec![]
    } else {
        vec![tokens[0]]
    }
}

/// Convert command tokens into an "always allow" pattern.
///
/// Uses the arity dictionary to extract the command prefix, then appends `*`
/// to create a wildcard pattern.
///
/// # Examples
///
/// ```
/// use flok_core::permission::arity::always_pattern;
///
/// assert_eq!(always_pattern(&["git", "commit", "-m", "fix"]), "git commit *");
/// assert_eq!(always_pattern(&["ls", "-la"]), "ls *");
/// assert_eq!(always_pattern(&["echo", "hello"]), "echo *");
/// ```
pub fn always_pattern(tokens: &[&str]) -> String {
    let prefix = command_prefix(tokens);
    if prefix.is_empty() {
        return "*".to_string();
    }
    format!("{} *", prefix.join(" "))
}

/// Tokenize a shell command string into words.
///
/// Simple whitespace-based tokenization. For more accurate parsing
/// (handling quotes, escapes), use tree-sitter-bash in the path module.
pub fn tokenize_command(command: &str) -> Vec<&str> {
    command.split_whitespace().collect()
}

// ---------------------------------------------------------------------------
// Arity dictionary
// ---------------------------------------------------------------------------

/// Maps command prefixes to the number of tokens that form the "command identity".
///
/// - Arity 1: `cat`, `ls`, `rm` — the command alone is the identity
/// - Arity 2: `git`, `npm`, `cargo` — command + subcommand
/// - Arity 3: `aws`, `gcloud`, `docker compose` — command + subcommand + sub-subcommand
static ARITY: LazyLock<HashMap<&'static str, usize>> = LazyLock::new(|| {
    let mut m = HashMap::new();

    // Arity 1 — simple Unix commands
    for cmd in [
        "cat", "cd", "chmod", "chown", "cp", "echo", "env", "export", "grep", "kill", "killall",
        "ln", "ls", "mkdir", "mv", "ps", "pwd", "rm", "rmdir", "sleep", "source", "tail", "touch",
        "unset", "which", "head", "less", "more", "wc", "sort", "uniq", "tr", "cut", "tee",
        "xargs", "find", "du", "df", "file", "stat", "uname", "date", "whoami", "id", "tree", "rg",
    ] {
        m.insert(cmd, 1);
    }

    // Arity 2 — command + subcommand
    for cmd in [
        "bazel",
        "brew",
        "bun",
        "cargo",
        "cdk",
        "cf",
        "cmake",
        "composer",
        "consul",
        "crictl",
        "deno",
        "docker",
        "eksctl",
        "firebase",
        "flyctl",
        "git",
        "go",
        "gradle",
        "helm",
        "heroku",
        "hugo",
        "ip",
        "kind",
        "kubectl",
        "kustomize",
        "make",
        "mc",
        "minikube",
        "mongosh",
        "mysql",
        "mvn",
        "ng",
        "npm",
        "nvm",
        "nx",
        "openssl",
        "pip",
        "pipenv",
        "pnpm",
        "poetry",
        "podman",
        "psql",
        "pulumi",
        "pyenv",
        "python",
        "rake",
        "rbenv",
        "redis-cli",
        "rustup",
        "serverless",
        "skaffold",
        "sls",
        "sst",
        "swift",
        "systemctl",
        "terraform",
        "tmux",
        "turbo",
        "ufw",
        "vault",
        "vercel",
        "volta",
        "wp",
        "yarn",
        "apt",
        "apt-get",
        "dnf",
        "yum",
        "pacman",
        "snap",
        "flatpak",
        "nix",
    ] {
        m.insert(cmd, 2);
    }

    // Arity 3 — command + subcommand + sub-subcommand
    for cmd in [
        "aws",
        "az",
        "bun run",
        "bun x",
        "cargo add",
        "cargo run",
        "consul kv",
        "deno task",
        "doctl",
        "docker builder",
        "docker compose",
        "docker container",
        "docker image",
        "docker network",
        "docker volume",
        "eksctl create",
        "gcloud",
        "gh",
        "git config",
        "git remote",
        "git stash",
        "ip addr",
        "ip link",
        "ip netns",
        "ip route",
        "kind create",
        "kubectl kustomize",
        "kubectl rollout",
        "npm exec",
        "npm init",
        "npm run",
        "npm view",
        "openssl req",
        "openssl x509",
        "pnpm dlx",
        "pnpm exec",
        "pnpm run",
        "podman container",
        "podman image",
        "pulumi stack",
        "sfdx",
        "terraform workspace",
        "vault auth",
        "vault kv",
        "yarn dlx",
        "yarn run",
    ] {
        m.insert(cmd, 3);
    }

    m
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_simple_commands() {
        assert_eq!(command_prefix(&["ls", "-la"]), vec!["ls"]);
        assert_eq!(command_prefix(&["cat", "file.txt"]), vec!["cat"]);
        assert_eq!(command_prefix(&["rm", "-rf", "dir"]), vec!["rm"]);
        assert_eq!(command_prefix(&["echo", "hello", "world"]), vec!["echo"]);
    }

    #[test]
    fn prefix_two_word_commands() {
        assert_eq!(command_prefix(&["git", "commit", "-m", "fix"]), vec!["git", "commit"]);
        assert_eq!(command_prefix(&["git", "status"]), vec!["git", "status"]);
        assert_eq!(command_prefix(&["npm", "install", "express"]), vec!["npm", "install"]);
        assert_eq!(command_prefix(&["cargo", "test", "--workspace"]), vec!["cargo", "test"]);
        assert_eq!(command_prefix(&["docker", "build", "."]), vec!["docker", "build"]);
    }

    #[test]
    fn prefix_three_word_commands() {
        assert_eq!(
            command_prefix(&["aws", "s3", "cp", "file.txt", "s3://bucket"]),
            vec!["aws", "s3", "cp"]
        );
        assert_eq!(
            command_prefix(&["docker", "compose", "up", "-d"]),
            vec!["docker", "compose", "up"]
        );
        assert_eq!(command_prefix(&["npm", "run", "dev"]), vec!["npm", "run", "dev"]);
        assert_eq!(
            command_prefix(&["git", "remote", "add", "origin", "url"]),
            vec!["git", "remote", "add"]
        );
    }

    #[test]
    fn prefix_unknown_command() {
        assert_eq!(command_prefix(&["my-custom-tool", "--flag", "value"]), vec!["my-custom-tool"]);
    }

    #[test]
    fn prefix_empty() {
        let empty: Vec<&str> = command_prefix(&[]);
        assert!(empty.is_empty());
    }

    #[test]
    fn prefix_single_token() {
        assert_eq!(command_prefix(&["git"]), vec!["git"]);
        assert_eq!(command_prefix(&["ls"]), vec!["ls"]);
    }

    #[test]
    fn always_pattern_examples() {
        assert_eq!(always_pattern(&["git", "commit", "-m", "fix"]), "git commit *");
        assert_eq!(always_pattern(&["ls", "-la"]), "ls *");
        assert_eq!(always_pattern(&["npm", "install", "express"]), "npm install *");
        assert_eq!(always_pattern(&["aws", "s3", "cp", "file"]), "aws s3 cp *");
        assert_eq!(always_pattern(&["echo", "hello"]), "echo *");
    }

    #[test]
    fn tokenize_basic() {
        assert_eq!(tokenize_command("git commit -m fix"), vec!["git", "commit", "-m", "fix"]);
        assert_eq!(tokenize_command("ls -la"), vec!["ls", "-la"]);
        assert_eq!(tokenize_command("echo hello world"), vec!["echo", "hello", "world"]);
    }

    #[test]
    fn tokenize_extra_whitespace() {
        assert_eq!(tokenize_command("  git   commit  "), vec!["git", "commit"]);
    }
}
