# fish completion for telegram-tui

complete -c telegram-tui -s h -l help -d 'Show help information'
complete -c telegram-tui -s v -l version -d 'Show version information'
complete -c telegram-tui -s c -l config -r -d 'Custom path to config.toml'
complete -c telegram-tui -l demo -d 'Run in offline demo mode with mock data'
complete -c telegram-tui -l single-column -d 'Force single column compact mode'
