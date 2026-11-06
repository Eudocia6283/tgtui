# fish completion for telegram-tui

set -l daemon_cmds run stop status install-service

complete -c telegram-tui -f
complete -c telegram-tui -s h -l help -d 'Show help information'
complete -c telegram-tui -s v -l version -d 'Show version information'
complete -c telegram-tui -s c -l config -r -F -d 'Custom path to config.toml'
complete -c telegram-tui -n 'not __fish_seen_subcommand_from daemon' -l chat -x -d 'Open this chat id on start'
complete -c telegram-tui -n 'not __fish_seen_subcommand_from daemon' -l no-daemon -d 'Run without the background service'
complete -c telegram-tui -n 'not __fish_seen_subcommand_from daemon' -l demo -d 'Run in offline demo mode with mock data'
complete -c telegram-tui -n 'not __fish_seen_subcommand_from daemon' -l single-column -d 'Force single column compact mode'

complete -c telegram-tui -n __fish_use_subcommand -a daemon -d 'Background service (Telegram connection + notifications)'
complete -c telegram-tui -n "__fish_seen_subcommand_from daemon; and not __fish_seen_subcommand_from $daemon_cmds" -a run -d 'Run the service in the foreground'
complete -c telegram-tui -n "__fish_seen_subcommand_from daemon; and not __fish_seen_subcommand_from $daemon_cmds" -a stop -d 'Stop the running service'
complete -c telegram-tui -n "__fish_seen_subcommand_from daemon; and not __fish_seen_subcommand_from $daemon_cmds" -a status -d 'Show service and login status'
complete -c telegram-tui -n "__fish_seen_subcommand_from daemon; and not __fish_seen_subcommand_from $daemon_cmds" -a install-service -d 'Write a systemd user unit'
