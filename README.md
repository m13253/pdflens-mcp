# pdflens-mcp

An MCP server for reading PDFs, coded by human, designed for AI.

## Provided tools

* `pdf_get_page_count`
* `pdf_to_text`
* `pdf_to_images`
* `list_mcp_root_paths`

## Usage

1.  Install Rust compiler: https://rustup.rs/

2.  Compile the code:

    ```bash
    cargo build --release
    ```

3.  Locate the program file at `./target/release/pdflens-mcp`.

4.  On your MCP client, add this MCP server.

    1. If your MCP client supports `mcp.json`:

        ```json
        {
            "mcpServers": {
                "pdflens": {
                    "command": "/path/to/pdflens-mcp/target/release/pdflens-mcp"
                }
            }
        }
        ```

    2. VS Code:

        ```bash
        code --add-mcp "{\"name\": \"pdflens\", \"command\": \"/path/to/pdflens-mcp/target/release/pdflens-mcp\"}"
        ```

    3. Continue.dev

        ```yaml
        mcpServers:
        - name: pdflens
            command: /path/to/pdflens-mcp/target/release/pdflens-mcp
        ```

    4. Codex

        ```toml
        [mcp_servers.pdflens]
        command = "/path/to/pdflens-mcp/target/release/pdflens-mcp"
        ```

## Path sandboxing

Pdflens is designed to only read PDFs located within the MCP root paths, which is usually the user’s workspace.

Each time before reading the PDFs, it checks the file path after resolving any symbolic links. If the PDF exists but is outside any MCP root paths, pdflens will return an error, asking the user to check the root path settings.

If your MCP client doesn’t specify a root path, pdflens will fallback to the current directory it is started in.

```json
{
    "mcpServers": {
        "pdflens": {
            "command": "/path/to/pdflens-mcp/target/release/pdflens-mcp",
            "cwd": "/path/to/workspace/if/root/path/is/unsupported"
        }
    }
}
```

## Not-vibe-coded declaration

This project is developed mainly with human effort. A minimal amount of Large Language Model (LLMs) assistance is used only for integration tests, autocompletion of repetitive glue code, and English grammar checking.
