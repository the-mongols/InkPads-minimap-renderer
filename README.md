# InkPads Minimap Renderer

This project was developed on Wargaming's request for the community. 

**Copyright © 2026 Wargaming.net**

This project is licensed under the Apache License, Version 2.0. A copy of the License can be found in the LICENSE file.

---

## Building the Renderer

To build the optimized release binary of the renderer, run the build helper script from the root directory:

### On Windows:
```cmd
build_renderer.bat
```

### On Linux:
```bash
chmod +x build_renderer.sh
./build_renderer.sh
```

## Running the CLI Renderer

The compiled binary will be placed at `target/release/minimap_renderer` (or `minimap_renderer.exe` on Windows). Run it with `--help` to view all available CLI arguments:

```bash
./target/release/minimap_renderer --help
```

For Discord bot integration, automated WoWs-Tournaments website rendering workflow, and service hosting setup instructions, please see the [Inkpads Bot README](inkpads-bot/README.md).

---

### Third-Party Licenses

This project builds upon the foundation of the `wows-toolkit` crates, which were developed by Lander Brandt under the MIT License.

**MIT License**

Copyright 2026 Lander Brandt

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the “Software”), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED “AS IS”, WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
