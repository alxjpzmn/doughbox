use clap::Parser;

use super::UsersCommand;

#[derive(Parser)]
struct TestCli {
    #[command(subcommand)]
    command: UsersCommand,
}

#[test]
fn account_commands_parse_explicit_console_authority() {
    assert_eq!(
        TestCli::try_parse_from(["users", "bootstrap"]).unwrap().command,
        UsersCommand::Bootstrap,
    );
    assert_eq!(
        TestCli::try_parse_from(["users", "recover", "--username", "Alice"]).unwrap().command,
        UsersCommand::Recover { username: "Alice".into(), enable: false },
    );
    assert_eq!(
        TestCli::try_parse_from(["users", "recover", "--username", "Alice", "--enable"]).unwrap().command,
        UsersCommand::Recover { username: "Alice".into(), enable: true },
    );
    assert!(TestCli::try_parse_from(["users", "recover"]).is_err());
    assert!(TestCli::try_parse_from(["users", "bootstrap", "--enable"]).is_err());
}
