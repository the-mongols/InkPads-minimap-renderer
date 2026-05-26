A monorepo of tools primarily used to render .wowsreplay files. Interacting with World of Warships game data, replays, and assets, a Discord bot has been setup to accept single and dual-renders of .wowsreplay files for common users, as well as KOTS-level of refereeing and broadcasting. Heatmaps for data visualization are underway as well, looking to provide KOTS broadcasts with interesting and usable stats, visualizations, and assets. A live markup (drawing tools) are underway via a webUI portal for similar purposes. 

**Developed by The_Mongols. Built upon the foundation of the wows-toolkit, wowsunpack, wows-replays, minimap-renderer, and replayshark community projects which utilize the MIT license.**



## Quick Start: Discord Bot

If you just want to get the Discord bot up and running:

1. **Run Setup**:
   *   **Windows**: Execute `setup_bot.bat`
   *   **Linux/macOS**: Execute `./setup_bot.sh` (after running `chmod +x setup_bot.sh`)
   This will install dependencies and create your `.env` file from the template.
2. **Configure**: Open `inkpads-bot/.env` and paste your Discord Bot Token.
3. **Launch**: Run `python inkpads-bot/bot.py` (Windows) or `python3 inkpads-bot/bot.py` (Linux).

For more detailed instructions, see the [Bot README](inkpads-bot/README.md).

## Licensing

This project is licensed under the Apache License, Version 2.0.

**Copyright © 2026 Wargaming.net**

*This project was developed on Wargaming's request for the community.*

See [LICENSE](LICENSE) and [NOTICE](NOTICE) for details.
