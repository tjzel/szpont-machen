# szpont features

szpont is a terminal UI and CLI that tracks the sessions of **Claude Code**, **Codex CLI** and **Kimi Code** on one machine.
The crate is in `szpont/`; the docs site (`docs/`) has the user-facing description of each feature. Decisions that are not
features are in [DECISIONS.md](DECISIONS.md); deferred ideas are in [FUTURE_IDEAS.md](FUTURE_IDEAS.md).

"Test" names how each feature is checked:

- **unit**: an automated test. Run them with `cargo test` in `szpont/`. The names are test function names.
- **manual**: checked by hand only, or not checked yet.

## Sessions

| Feature | Main source | Test |
| --- | --- | --- |
| One list for Claude Code, Codex CLI and Kimi Code sessions, read from each tool's own store | `szpont/src/adapters/` | unit: `claude.rs`, `codex.rs`, `kimi.rs` tests |
| Live status per session: `RUNNING`, `BLOCKED` (Claude Code only), `IDLE` | `szpont/src/adapters/` (`liveness`) | unit: `fresh_wire_activity_reports_running_even_with_stale_state` (Kimi); manual for Claude Code and Codex |
| Incremental parsing: after the first scan, a refresh reads only appended data | `szpont/src/adapters/claude.rs`, `szpont/src/store/checkpoints.rs` | unit: `incremental_parse_with_ring_skips_old_request_ids`, `incomplete_tail_line_is_not_consumed`, `tail_context_survives_multibyte_split_at_tail_boundary` |
| The session directory is the last `cwd` in the transcript; a stored directory that was deleted is replaced by a newer one that exists | `szpont/src/adapters/claude.rs`, `szpont/src/core/snapshot.rs` | unit: `last_cwd_in_transcript_wins`, `incremental_parse_updates_cwd_and_keeps_it_when_absent`, `newer_existing_cwd_replaces_a_stored_deleted_cwd` |
| Local scope: in a git repository szpont shows that repository's sessions, in a plain directory it scopes by path; `p` toggles local and global; `--global` picks only the first screen | `szpont/src/core/repo.rs`, `szpont/src/app/mod.rs` | unit: `a_directory_without_git_still_yields_a_scopeable_context`, `origin_forms_normalize_to_same_key`, `first_empty_repo_snapshot_falls_back_to_monitor`, `forced_repo_view_is_not_flipped_by_an_empty_snapshot` |
| Projects: `s` gives the selected session's directory a project name; several directories can share one; the PROJECT column shows it in a colour taken from the name; local scope follows the project | `szpont/src/app/mod.rs`, `szpont/src/app/screens/monitor.rs` | unit: `assigning_a_project_persists_and_labels_every_matching_row`, `two_directories_can_share_one_project_and_scope_prefers_it`, `project_colors_are_stable_and_spread_across_the_palette`, `a_project_name_keeps_its_colour_across_versions` |
| Refresh: file changes start a scan, an idle scan runs every 15 s (`--refresh-secs`), `r` forces one. A snapshot that changes the row order waits for `r`; the cursor stays on the selected session | `szpont/src/scanner.rs`, `szpont/src/app/mod.rs` | unit: `reordered_snapshot_is_held_until_refresh_key`, `same_order_snapshot_applies_in_place`, `refresh_flag_forces_snapshot_apply`, `refresh_keeps_the_cursor_on_the_selected_session`, `selection_falls_back_when_the_selected_session_disappears` |
| Sessions that the tools no longer list are pruned after a grace period | `szpont/src/core/snapshot.rs` | unit: `prune_respects_the_grace_period_and_null_last_seen`, `discovered_sessions_get_their_last_seen_stamped` |

## Launching

| Feature | Main source | Test |
| --- | --- | --- |
| `Enter` resumes the selected session with its tool. szpont replaces itself with the tool, so leaving the tool returns to the shell. Inside tmux the session opens in a new tmux window instead | `szpont/src/app/run.rs` (`launch_style`) | unit: `enter_replaces_szpont_unless_tmux_can_give_it_a_window`, `enter_leaves_szpont_and_shift_e_comes_back_to_it`, `tmux_new_window_args_set_name_cwd_and_escaped_command` |
| `E` resumes in this window and returns to szpont when the tool exits | `szpont/src/app/run.rs` (`suspend_and_run`) | unit: `enter_leaves_szpont_and_shift_e_comes_back_to_it` |
| `n` starts a new session (tool picker; `1`–`9` picks a tool). It launches like `Enter` | `szpont/src/app/mod.rs` (`launch_new_session`) | unit: `a_new_session_also_leaves_szpont` |
| `Enter` refuses a session archived inside its tool and says to press `u` | `szpont/src/app/mod.rs` | unit: `enter_refuses_to_resume_a_natively_archived_session` |
| Arguments of a tmux launch are shell-escaped | `szpont/src/app/run.rs` (`shell_escape`) | unit: `shell_escape_passes_safe_args_and_quotes_the_rest` |

## Usage and limits

| Feature | Main source | Test |
| --- | --- | --- |
| Token usage per session, subagents included; a repeated request id is counted once | `szpont/src/adapters/claude.rs`, `szpont/src/adapters/kimi.rs` | unit: `duplicated_request_id_counted_once`, `wire_usage_records_are_summed` |
| Context window use per session in the `CTX` column; `o` sets the window per model when the tool does not report it | `szpont/src/app/screens/options.rs`, `szpont/src/app/mod.rs` | unit: `options_screen_sets_persistent_context_window`, `token_count_parsing` |
| Rate limits per tool from the providers' usage endpoints, including model-scoped Claude windows (for example the weekly Fable limit) | `szpont/src/limits/` | unit: `structured_limits_array_wins_and_carries_model_scoped_windows`, `full_body_produces_all_windows_and_gates_extra_on_is_enabled`, `usage_and_limit_entries_produce_windows`, `last_rate_limits_takes_the_newest_token_count_line` |
| Limits have their own header row, with a countdown to each reset; `l` opens the limits popup | `szpont/src/app/screens/monitor.rs` | unit: `limits_render_on_their_own_row_with_a_reset_countdown`, `humanize_window_boundaries` |
| Expired or missing sign-ins are reported with the tool and the age | `szpont/src/limits/mod.rs` | unit: `credential_states_explain_themselves_per_tool` |
| The limits cache is shared in the database and refetched when another szpont version wrote it | `szpont/src/limits/mod.rs` | unit: `a_cache_written_by_another_version_is_refetched_not_reused` |
| A Kimi base URL override must be https and a known host | `szpont/src/limits/kimi_oauth.rs` | unit: `base_url_override_requires_https_and_a_known_host`, `base_url_override_rejects_authority_delimiter_smuggling` |

## Organizing sessions

| Feature | Main source | Test |
| --- | --- | --- |
| `c` marks sessions as completed (marking mode, `Enter` confirms, `Esc` or `q` cancels); `a` opens the archive screen | `szpont/src/app/mod.rs` | unit: `c_enters_marking_mode_and_enter_completes_marked`, `esc_cancels_marking_mode_without_completing`, `q_does_not_quit_but_cancels_marking_mode`, `esc_leaves_archive` |
| `u` reopens a session; for a session archived inside Codex it runs `codex unarchive`; other tools say where to unarchive | `szpont/src/app/mod.rs`, `szpont/src/adapters/codex.rs` | unit: `u_asks_the_tool_to_unarchive_a_natively_archived_session`, `u_on_a_tool_without_unarchive_support_says_where_to_unarchive`, `unarchive_passes_only_the_session_id` |
| `d` deletes sessions from szpont and from the tool's storage (`codex delete --force`; file removal for Claude Code and Kimi). Open sessions are skipped | `szpont/src/app/mod.rs`, `szpont/src/adapters/` | unit: `d_marks_and_enter_deletes_session_and_tool_files`, `delete_skips_sessions_that_are_still_open`, `delete_is_forced_because_codex_cannot_prompt_without_a_terminal`, `delete_session_refuses_paths_outside_the_sessions_root` |
| `m` renames a session (szpont-side title that survives rescans) | `szpont/src/app/mod.rs`, `szpont/src/store/mod.rs` | unit: `m_renames_session_with_sticky_custom_title`, `rename_survives_a_snapshot_scanned_before_it_landed`, `custom_title_survives_later_auto_upsert` |
| `v` tree view: sessions nested by directory, from the monitor, repo and archive screens | `szpont/src/app/mod.rs` | unit: `v_toggles_tree_view_grouped_by_location`, `tree_from_archive_shows_completed_sessions`, `tree_view_order_groups_filesystem_roots_together`, `tree_components_depth_is_capped` |
| `f` fuzzy find over title, prompt, path and tool; `Tab` / `Shift+Tab` cycle the tool filter | `szpont/src/app/mod.rs` | unit: `fuzzy_filter_is_live_and_ranked`, `fuzzy_scoring_prefers_consecutive_and_word_starts` |
| `i` session detail popup | `szpont/src/app/screens/monitor.rs` | manual |

## Screen and input

| Feature | Main source | Test |
| --- | --- | --- |
| The footer lists every key the current screen binds and reserves the rows a word wrap needs; a status message does not hide the hints | `szpont/src/app/screens/monitor.rs` | unit: `every_key_that_does_something_is_advertised_in_the_footer`, `every_key_hint_is_visible_in_every_state`, `a_status_message_does_not_hide_key_hints`, `the_reserved_height_matches_what_ratatui_actually_wraps` |
| The version shows next to the title | `szpont/src/app/screens/monitor.rs` | unit: `the_version_shows_next_to_the_title_in_both_header_modes` |
| Title and repo columns share the free width; the title takes only what its longest entry needs | `szpont/src/app/screens/monitor.rs` | unit: `title_and_repo_share_the_leftover_width_evenly_when_both_want_it`, `the_title_column_shrinks_to_the_longest_title_and_repo_takes_the_slack` |
| Line editor: word and line deletion, a cursor that does not shift text, a capped buffer; Ctrl, Alt and Cmd combinations do not act as plain letters | `szpont/src/app/mod.rs` | unit: `line_editor_word_and_line_deletion_shortcuts`, `line_editor_cursor_spans_do_not_shift_text`, `line_editor_caps_its_buffer_and_seed`, `line_editor_ignores_modified_chars_instead_of_inserting` |
| Quitting needs a double `Ctrl+C` | `szpont/src/app/mod.rs` | unit: `ctrl_c_needs_double_press` |
| Control and bidi characters from transcripts are replaced before display | `szpont/src/core/mod.rs` | unit: `sanitize_replaces_control_and_bidi_characters` |

## CLI and agents

| Feature | Main source | Test |
| --- | --- | --- |
| `szpont sessions` (with `--json`, `--all`, `--repo`), `complete`, `reopen`, `limits` for scripts | `szpont/src/cli.rs`, `szpont/src/commands.rs` | manual |
| Shell completions (`szpont completions <shell>`) | `szpont/src/cli.rs` | manual |
| MCP server (`szpont mcp`) with `report_session_start`, `mark_session_completed` and `reopen_session`; the initialize result carries the instructions for agents | `szpont/src/mcp/` | unit: `tools_list_reports_the_exact_tool_set`, `initialize_documents_every_advertised_tool`, `mark_and_reopen_round_trip_through_the_store`, `report_session_start_does_not_overwrite_a_stored_cwd`, `malformed_session_id_is_rejected` |
| `szpont install-mcp` registers the MCP server with the tools | `szpont/src/mcp/install.rs` | manual |
| `szpont report-start` registers a session from any harness; `--claude-hook` reads a Claude Code SessionStart payload and always exits 0 | `szpont/src/commands.rs`, `szpont/src/session_resolve.rs` | manual |
| Hidden `--session-allowlist` limits the list to chosen sessions with aliases, for demo recordings (`scripts/demo`) | `szpont/src/allowlist.rs`, `szpont/src/core/snapshot.rs` | unit: `loads_enabled_entries_and_aliases`, `allowlist_filters_before_usage_and_applies_aliases`, `allowlisted_rows_search_and_group_by_alias_without_real_path` |

## Storage

| Feature | Main source | Test |
| --- | --- | --- |
| One SQLite database shared by every TUI, CLI and MCP process; scans take the write lock per session | `szpont/src/store/` | unit: `concurrent_read_then_write_txns_serialize` |
| The database and the directories szpont creates are private (0600 / 0700); an existing database is opened only by SQLite | `szpont/src/store/mod.rs` | unit: `store_files_and_created_directories_are_private`, `an_existing_database_file_is_never_opened_behind_sqlites_back`, `an_existing_database_file_is_still_forced_private` |
| Schema migrations keep existing rows | `szpont/src/store/schema.rs`, `szpont/src/store/mod.rs` | unit: `v2_database_is_migrated_without_losing_rows`, `open_is_idempotent_on_a_current_database`, `legacy_merge_fills_gaps_and_keeps_local_values` |
