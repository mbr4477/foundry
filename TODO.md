# Claude Jr

People recommend treating Claude Code as a junior developer. The goal of this project is to interact with Claude Code exclusively through a locally-hosted Gitea instance.
When assigned issues, Claude Code should plan and brainstorm them, interacting with the issue reporter before being approved to implement changes, creating a branch, and opening a pull request when the tested code is ready for review. Claude Code should interact with all reviewers to respond to their reviews and update the code in the pull request.

Help me plan a project to accomplish this. Critique my ideas and design an architecture that is maintainable and organized.

## Project Guidelines

- Write project code in Rust.
- Claude Code should always run in a Docker container for isolation.
- Claude Code containers should be ephemeral with persistent session and configuration in Docker volumes.
- The containers should use environment variables for configuring the working repository and any auth tokens.
- The Gitea instance should have a dedicated agent user.
- Gitea setup should have a script to initialize users and any hooks.

