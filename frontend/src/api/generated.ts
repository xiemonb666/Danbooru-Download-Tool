export interface paths {
    "/api/config": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["get_config"];
        put: operations["update_config"];
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/danbooru/autocomplete": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["danbooru_autocomplete"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/danbooru/count": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["danbooru_count"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/danbooru/posts": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["danbooru_posts"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/danbooru/posts/{id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["danbooru_post"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/danbooru/posts/{id}/media/{variant}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["danbooru_media"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/diagnostics": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["diagnostics"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/downloads/history": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["download_history"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/health": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["health"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/library/facets": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["library_facets"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/library/items": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["list_library_items"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/library/items/{id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["library_item"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/library/media/{id}/{variant}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["library_media"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/library/quarantine": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["list_quarantine"];
        put?: never;
        post?: never;
        delete: operations["purge_quarantine"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/library/quarantine/{id}/restore": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["restore_quarantine"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/library/roots": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["list_roots"];
        put?: never;
        post: operations["create_root"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/library/roots/{id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put: operations["update_root"];
        post?: never;
        delete: operations["delete_root"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/library/roots/{id}/directories": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["list_root_directories"];
        put?: never;
        post: operations["create_root_directory"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/secrets/{kind}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put: operations["put_secret"];
        post?: never;
        delete: operations["delete_secret"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/tasks": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["list_tasks"];
        put?: never;
        post: operations["create_task"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/tasks/events": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["task_events"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/tasks/{id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["task_detail"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/tasks/{id}/{action}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["task_action"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/training/adapters": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["training_adapters"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/training/datasets/augmentations": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["training_gallery_augmentations"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/training/datasets/gallery": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["training_gallery_dataset"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/training/gpus": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["training_gpus"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/training/lora-svd/analyses": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["create_lora_svd_analysis"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/training/lora-svd/analyses/{id}/export": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["export_lora_svd_analysis"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/training/lora-svd/analyses/{id}/modules/{module_id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["lora_svd_module"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/training/paths": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["training_path_browser"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/training/preflight": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["training_preflight"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/training/presets": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["list_training_presets"];
        put?: never;
        post: operations["create_training_preset"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/training/presets/import": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["import_training_preset"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/training/presets/{id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put: operations["update_training_preset"];
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/training/presets/{id}/export": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["export_training_preset"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/training/presets/{id}/toml": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put: operations["update_training_preset_from_toml"];
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/training/preview": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["training_preview"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/training/queue": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["training_queue"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/training/runtime-profiles": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["training_runtime_profiles"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/training/runtime-profiles/{id}/diagnostics": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["training_runtime_diagnostics"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/training/runtime-profiles/{id}/install": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["install_training_runtime"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/training/tasks/{id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post?: never;
        delete: operations["delete_training_task"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/training/tasks/{id}/artifacts": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["training_artifacts"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/training/tasks/{id}/artifacts/{artifact_id}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["training_artifact_file"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/training/tasks/{id}/cleanup-preview": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["training_cleanup_preview"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/training/tasks/{id}/events": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["training_events"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/training/tasks/{id}/logs": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["training_logs"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/training/tasks/{id}/metrics": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["training_metrics"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/training/tasks/{id}/metrics/overview": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["training_metrics_overview"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/vision-crop/runtime-profiles/{id}/health": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["vision_crop_runtime_health"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/vision-crop/runtime-profiles/{id}/install": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["install_vision_crop_runtime"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/vllm/health": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get: operations["vllm_health"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/vllm/load": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["vllm_load"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/vllm/unload": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post: operations["vllm_unload"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
}
export type webhooks = Record<string, never>;
export interface components {
    schemas: {
        ApiErrorBody: {
            code: string;
            fields?: {
                [key: string]: unknown;
            } | null;
            message: string;
            retryable: boolean;
        };
        ApiFailure: {
            error: components["schemas"]["ApiErrorBody"];
            request_id: string;
        };
        ApiSuccess_AppConfig: {
            data: {
                background_image: string;
                /** Format: int32 */
                background_opacity: number;
                blur_sensitive_media: boolean;
                danbooru_api_key_configured: boolean;
                danbooru_username: string;
                download_concurrency: number;
                filename_template: string;
                proxy_url?: string | null;
                ugoira_policy: components["schemas"]["UgoiraPolicy"];
                vllm_allowed_hosts: string[];
                vllm_api_key_configured: boolean;
                vllm_base_url: string;
                vllm_concurrency: number;
                vllm_language: components["schemas"]["VllmLanguage"];
                vllm_max_length: number;
                vllm_max_tags: number;
                vllm_model: string;
                vllm_reference_existing: boolean;
                vllm_system_prompt: string;
                vllm_tag_mode: components["schemas"]["VllmTagMode"];
                vllm_verify_danbooru: boolean;
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_DanbooruCount: {
            data: {
                /** Format: int64 */
                count: number;
                exact: boolean;
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_DanbooruPost: {
            data: {
                downloaded: boolean;
                /** Format: double */
                duration?: number | null;
                /** Format: int64 */
                fav_count: number;
                file_ext: string;
                /** Format: int64 */
                file_size: number;
                /** Format: int64 */
                id: number;
                /** Format: int32 */
                image_height: number;
                /** Format: int32 */
                image_width: number;
                is_ugoira: boolean;
                is_video: boolean;
                rating: components["schemas"]["ContentRating"];
                restricted: boolean;
                /** Format: int64 */
                score: number;
                source?: string | null;
                tags: components["schemas"]["DanbooruTags"];
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_DanbooruPostsPage: {
            data: {
                next_page?: string | null;
                /** Format: int64 */
                page: number;
                posts: components["schemas"]["DanbooruPost"][];
                previous_page?: string | null;
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_DownloadHistoryPage: {
            data: {
                items: components["schemas"]["DownloadHistoryRecord"][];
                next_cursor?: string | null;
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_HealthStatus: {
            data: {
                database: components["schemas"]["DatabaseHealthState"];
                status: components["schemas"]["HealthState"];
                /** Format: int64 */
                uptime_seconds: number;
                version: string;
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_LibraryFacets: {
            data: {
                /** Format: int64 */
                catalog_revision: number;
                resolution_ranges: components["schemas"]["LibraryResolutionRange"][];
                score_ranges: components["schemas"]["LibraryScoreRange"][];
                /** Format: int64 */
                total: number;
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_LibraryPage: {
            data: {
                /** Format: int64 */
                catalog_revision: number;
                items: components["schemas"]["LocalMedia"][];
                next_cursor?: string | null;
                /** Format: int64 */
                page: number;
                previous_cursor?: string | null;
                resolution_ranges: components["schemas"]["LibraryResolutionRange"][];
                score_ranges: components["schemas"]["LibraryScoreRange"][];
                /** Format: int64 */
                total: number;
                /** Format: int64 */
                total_pages: number;
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_LocalMedia: {
            data: {
                created_at: string;
                /** Format: double */
                duration?: number | null;
                filename: string;
                /** Format: int32 */
                height?: number | null;
                id: string;
                mime_type: string;
                post_created_at?: string | null;
                /** Format: int64 */
                post_id?: number | null;
                rating?: null | components["schemas"]["ContentRating"];
                relative_path: string;
                root_id: string;
                /** Format: int64 */
                size_bytes: number;
                tags: string[];
                /** Format: int32 */
                width?: number | null;
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_LoraSvdAnalysisResult: {
            data: {
                comparison?: null | components["schemas"]["LoraSvdComparison"];
                execution: components["schemas"]["LoraSvdExecution"];
                /** Format: int64 */
                expires_at: number;
                id: string;
                reports: components["schemas"]["LoraSvdModelReport"][];
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_LoraSvdModule: {
            data: {
                /** Format: double */
                alpha: number;
                component: string;
                effective_rank: components["schemas"]["LoraSvdThresholdRanks"];
                /** Format: double */
                energy: number;
                flag?: string | null;
                id: string;
                /** Format: int64 */
                numerical_rank: number;
                /** Format: int64 */
                rank: number;
                /** Format: double */
                scale: number;
                /** @description Returned from the module detail/export endpoints; omitted from the initial summary. */
                singular_values?: number[] | null;
                /** Format: double */
                stable_rank: number;
                /** Format: double */
                tail_energy_20: number;
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_MediaDirectory: {
            data: {
                relative_path: string;
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_MediaDirectoryList: {
            data: {
                directories: string[];
                truncated: boolean;
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_MediaRoot: {
            data: {
                created_at: string;
                id: string;
                indexed: boolean;
                linux_path?: string | null;
                media_count: number;
                name: string;
                updated_at: string;
                windows_path?: string | null;
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_PurgeResponse: {
            data: {
                purged: number;
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_QuarantineEntry: {
            data: {
                created_at: string;
                id: string;
                original_relative_path: string;
                quarantine_relative_path: string;
                reason: string;
                root_id: string;
                /** Format: int64 */
                size_bytes: number;
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_RootRemoval: {
            data: {
                id: string;
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_SecretResponse: {
            data: {
                configured: boolean;
                storage: components["schemas"]["SecretStorage"];
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_SystemDiagnostics: {
            data: {
                active_workers: number;
                database_pool: components["schemas"]["DatabasePoolDiagnostics"];
                queued_tasks: number;
                scheduler: {
                    [key: string]: components["schemas"]["SchedulerResourceDiagnostics"];
                };
                thumbnail_cache: components["schemas"]["ThumbnailCacheDiagnostics"];
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_TaskDetails: {
            data: {
                item_counts: components["schemas"]["TaskItemCounts"];
                items: components["schemas"]["TaskItem"][];
                next_cursor?: string | null;
                result?: unknown;
                task: components["schemas"]["TaskSummary"];
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_TaskSnapshot: {
            data: {
                /** Format: int64 */
                last_event_id: number;
                tasks: components["schemas"]["TaskSummary"][];
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_TaskSummary: {
            data: {
                created_at: string;
                failures: components["schemas"]["TaskFailure"][];
                id: string;
                kind: components["schemas"]["TaskKind"];
                preview?: null | components["schemas"]["TaskPreview"];
                progress: components["schemas"]["TaskProgress"];
                /** Format: int64 */
                revision: number;
                scheduling: components["schemas"]["TaskScheduling"];
                stage: string;
                status: components["schemas"]["TaskStatus"];
                title: string;
                training?: null | components["schemas"]["TrainingTaskSummary"];
                updated_at: string;
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_TrainingArtifactsResponse: {
            data: {
                artifacts: components["schemas"]["TrainingArtifact"][];
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_TrainingAugmentationDiscoveryResponse: {
            data: {
                source: components["schemas"]["TrainingGalleryDatasetResponse"];
                subsets: components["schemas"]["TrainingAugmentationSubsetResponse"][];
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_TrainingCleanupPreviewResponse: {
            data: {
                deletable: components["schemas"]["TrainingCleanupPath"][];
                retained: components["schemas"]["TrainingCleanupPath"][];
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_TrainingCleanupResponse: {
            data: {
                deleted: components["schemas"]["TrainingCleanupPath"][];
                retained: components["schemas"]["TrainingCleanupPath"][];
                task_id: string;
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_TrainingGalleryDatasetResponse: {
            data: {
                /** Format: int64 */
                caption_count: number;
                caption_extension: string;
                /** Format: int64 */
                effective_image_count: number;
                /** Format: int64 */
                image_count: number;
                image_dir: string;
                relative_directory: string;
                /** Format: int32 */
                repeats: number;
                root_id: string;
                root_name: string;
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_TrainingLogsResponse: {
            data: {
                /** Format: int64 */
                cursor: number;
                text: string;
                truncated: boolean;
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_TrainingMetricsOverviewResponse: {
            data: {
                /** Format: int64 */
                cursor: number;
                series: components["schemas"]["TrainingMetricSeriesSummary"][];
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_TrainingMetricsResponse: {
            data: {
                /** Format: int64 */
                cursor: number;
                metrics: components["schemas"]["TrainingMetric"][];
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_TrainingPathBrowserResponse: {
            data: {
                current_path: string;
                directories: components["schemas"]["TrainingPathEntry"][];
                files: components["schemas"]["TrainingPathEntry"][];
                parent_path?: string | null;
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_TrainingPreflightResponse: {
            data: {
                checks: components["schemas"]["TrainingPreflightCheck"][];
                /** Format: int64 */
                effective_steps: number;
                /** Format: int64 */
                estimated_vram_mib: number;
                ready: boolean;
                suggestions: components["schemas"]["TrainingParameterSuggestion"][];
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_TrainingPresetExportResponse: {
            data: {
                name: string;
                toml: string;
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_TrainingPresetResponse: {
            data: {
                /** Format: int64 */
                created_at: number;
                id: string;
                name: string;
                training: components["schemas"]["TrainingRunRequest"];
                /** Format: int64 */
                updated_at: number;
                version_count: number;
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_TrainingPreviewResponse: {
            data: {
                toml: string;
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_TrainingQueueResponse: {
            data: {
                entries: components["schemas"]["TrainingQueueEntry"][];
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_TrainingRuntimeDiagnostics: {
            data: {
                checks: components["schemas"]["TrainingRuntimeCheck"][];
                profile: components["schemas"]["TrainingRuntimeProfile"];
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_TrainingRuntimeProfile: {
            data: {
                id: string;
                installed: boolean;
                installing: boolean;
                kind: string;
                label: string;
                last_error?: string | null;
                managed: boolean;
                python_path: string;
                runtime_root: string;
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_Vec_MediaRoot: {
            data: {
                created_at: string;
                id: string;
                indexed: boolean;
                linux_path?: string | null;
                media_count: number;
                name: string;
                updated_at: string;
                windows_path?: string | null;
            }[];
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_Vec_QuarantineEntry: {
            data: {
                created_at: string;
                id: string;
                original_relative_path: string;
                quarantine_relative_path: string;
                reason: string;
                root_id: string;
                /** Format: int64 */
                size_bytes: number;
            }[];
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_Vec_TagSuggestion: {
            data: {
                category: components["schemas"]["TagCategory"];
                label: string;
                /** Format: int64 */
                post_count?: number | null;
                value: string;
            }[];
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_Vec_TrainingAdapterResponse: {
            data: {
                family: string;
                family_label: string;
                fields: components["schemas"]["TrainingAdapterField"][];
                groups: components["schemas"]["TrainingAdapterGroup"][];
                id: string;
                label: string;
                trainer: string;
                training_type: string;
                training_type_label: string;
                version: string;
            }[];
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_Vec_TrainingGpu: {
            data: {
                external_processes: components["schemas"]["TrainingGpuExternalProcess"][];
                /** Format: int64 */
                fan_speed_percent?: number | null;
                /** Format: int64 */
                graphics_clock_mhz?: number | null;
                id: string;
                /** Format: int64 */
                memory_clock_mhz?: number | null;
                /** Format: int64 */
                memory_total_mib: number;
                /** Format: int64 */
                memory_used_mib: number;
                name: string;
                /** Format: double */
                power_draw_w?: number | null;
                /** Format: double */
                power_limit_w?: number | null;
                /** Format: int64 */
                temperature_c?: number | null;
                /** Format: int64 */
                utilization_percent: number;
            }[];
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_Vec_TrainingPresetResponse: {
            data: {
                /** Format: int64 */
                created_at: number;
                id: string;
                name: string;
                training: components["schemas"]["TrainingRunRequest"];
                /** Format: int64 */
                updated_at: number;
                version_count: number;
            }[];
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_Vec_TrainingRuntimeProfile: {
            data: {
                id: string;
                installed: boolean;
                installing: boolean;
                kind: string;
                label: string;
                last_error?: string | null;
                managed: boolean;
                python_path: string;
                runtime_root: string;
            }[];
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_VisionCropRuntimeHealth: {
            data: {
                gpu_id: string;
                gpu_name?: string | null;
                installing: boolean;
                last_error?: string | null;
                message: string;
                models_ready: boolean;
                providers: string[];
                python_path: string;
                ready: boolean;
                runtime_profile_id: string;
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_VllmHealthStatus: {
            data: {
                available: boolean;
                message: string;
                models: string[];
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_VllmLoadStatus: {
            data: {
                message: string;
                state: string;
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        ApiSuccess_VllmUnloadStatus: {
            data: {
                message: string;
                state: string;
            };
            meta?: {
                [key: string]: unknown;
            } | null;
        };
        AppConfig: {
            background_image: string;
            /** Format: int32 */
            background_opacity: number;
            blur_sensitive_media: boolean;
            danbooru_api_key_configured: boolean;
            danbooru_username: string;
            download_concurrency: number;
            filename_template: string;
            proxy_url?: string | null;
            ugoira_policy: components["schemas"]["UgoiraPolicy"];
            vllm_allowed_hosts: string[];
            vllm_api_key_configured: boolean;
            vllm_base_url: string;
            vllm_concurrency: number;
            vllm_language: components["schemas"]["VllmLanguage"];
            vllm_max_length: number;
            vllm_max_tags: number;
            vllm_model: string;
            vllm_reference_existing: boolean;
            vllm_system_prompt: string;
            vllm_tag_mode: components["schemas"]["VllmTagMode"];
            vllm_verify_danbooru: boolean;
        };
        /** @enum {string} */
        ArtistPrefix: "artist" | "at";
        BatchDownloadFilter: {
            exclude_tags: string[];
            include_tags: string[];
            /** Format: int32 */
            minimum_resolution: number;
            /** Format: int64 */
            minimum_score: number;
        };
        ClTaggerRetagTaskOptions: {
            /** Format: float */
            character_threshold?: number | null;
            /** Format: float */
            copyright_threshold?: number | null;
            /** Format: float */
            general_threshold?: number | null;
            /** Format: int32 */
            max_tags?: number | null;
            model_path?: string | null;
            /** Format: float */
            quality_threshold?: number | null;
        };
        /** @enum {string} */
        ContentRating: "g" | "s" | "q" | "e" | "unknown";
        CreateMediaDirectoryRequest: {
            relative_path: string;
        };
        CreateTaskRequest: components["schemas"]["DownloadTaskRequest"] | components["schemas"]["IndexLibraryTaskRequest"] | components["schemas"]["ReindexLibraryTaskRequest"] | components["schemas"]["IntegrityScanTaskRequest"] | components["schemas"]["ExactDedupTaskRequest"] | components["schemas"]["NearDedupTaskRequest"] | components["schemas"]["ResizeTaskRequest"] | components["schemas"]["HeicConvertTaskRequest"] | components["schemas"]["DeleteByTagTaskRequest"] | components["schemas"]["DeleteSelectedTaskRequest"] | components["schemas"]["TagPipelineTaskRequest"] | components["schemas"]["VllmTagTaskRequest"] | components["schemas"]["DatasetAugmentationTaskRequest"] | components["schemas"]["TrainingTaskRequest"];
        DanbooruCount: {
            /** Format: int64 */
            count: number;
            exact: boolean;
        };
        DanbooruPost: {
            downloaded: boolean;
            /** Format: double */
            duration?: number | null;
            /** Format: int64 */
            fav_count: number;
            file_ext: string;
            /** Format: int64 */
            file_size: number;
            /** Format: int64 */
            id: number;
            /** Format: int32 */
            image_height: number;
            /** Format: int32 */
            image_width: number;
            is_ugoira: boolean;
            is_video: boolean;
            rating: components["schemas"]["ContentRating"];
            restricted: boolean;
            /** Format: int64 */
            score: number;
            source?: string | null;
            tags: components["schemas"]["DanbooruTags"];
        };
        DanbooruPostsPage: {
            next_page?: string | null;
            /** Format: int64 */
            page: number;
            posts: components["schemas"]["DanbooruPost"][];
            previous_page?: string | null;
        };
        DanbooruTags: {
            artist: string[];
            character: string[];
            copyright: string[];
            general: string[];
            meta: string[];
        };
        /** @enum {string} */
        DatabaseHealthState: "ok";
        DatabasePoolDiagnostics: {
            capacity: number;
            idle: number;
            in_use: number;
            total: number;
        };
        DatasetAugmentationTaskOptions: {
            excluded_media_ids?: string[] | null;
            horizontal_flip?: boolean | null;
            /** Format: int32 */
            jpeg_quality?: number | null;
            /** Format: int64 */
            library_post_created_from?: number | null;
            /** Format: int64 */
            library_post_created_to?: number | null;
            library_query?: string | null;
            library_relative_directory?: string | null;
            media_ids?: string[] | null;
            /** Format: int32 */
            min_long_side?: number | null;
            /** Format: double */
            min_megapixels?: number | null;
            /** Format: int32 */
            min_short_side?: number | null;
            output_directory?: string | null;
            relative_directory?: string | null;
            retagging?: null | components["schemas"]["DerivedRetaggingTaskOptions"];
            smart_crop?: null | components["schemas"]["SmartCropTaskOptions"];
            /** Format: int32 */
            test_percent?: number | null;
            /** Format: int32 */
            train_percent?: number | null;
            /** Format: int32 */
            validation_percent?: number | null;
        };
        DatasetAugmentationTaskRequest: {
            options: components["schemas"]["DatasetAugmentationTaskOptions"];
            root_id: string;
            /**
             * @description discriminator enum property added by openapi-typescript
             * @enum {string}
             */
            type: "dataset_augmentation";
        };
        /** @enum {string} */
        DatasetAugmentationTaskType: "dataset_augmentation";
        DeleteByTagTaskOptions: {
            preflight?: boolean | null;
            relative_directory?: string | null;
            tag: string;
        };
        DeleteByTagTaskRequest: {
            options: components["schemas"]["DeleteByTagTaskOptions"];
            root_id: string;
            /**
             * @description discriminator enum property added by openapi-typescript
             * @enum {string}
             */
            type: "delete_by_tag";
        };
        /** @enum {string} */
        DeleteByTagTaskType: "delete_by_tag";
        DeleteSelectedTaskRequest: {
            options: components["schemas"]["MediaIdsTaskOptions"];
            root_id: string;
            /**
             * @description discriminator enum property added by openapi-typescript
             * @enum {string}
             */
            type: "delete_selected";
        };
        /** @enum {string} */
        DeleteSelectedTaskType: "delete_selected";
        /** @enum {string} */
        DerivedRetaggingMode: "vllm" | "cl_tagger";
        DerivedRetaggingTaskOptions: {
            cl_tagger?: null | components["schemas"]["ClTaggerRetagTaskOptions"];
            mode?: null | components["schemas"]["DerivedRetaggingMode"];
            preserve_artist_character_tags?: boolean | null;
            send_to_vllm?: boolean | null;
            vllm?: null | components["schemas"]["VllmRetagTaskOptions"];
        };
        DownloadHistoryPage: {
            items: components["schemas"]["DownloadHistoryRecord"][];
            next_cursor?: string | null;
        };
        DownloadHistoryRecord: {
            /** Format: int64 */
            bytes_processed: number;
            can_repeat: boolean;
            /** Format: int64 */
            completed_items: number;
            created_at: string;
            /** Format: int64 */
            duration_seconds?: number | null;
            error_message?: string | null;
            /** Format: int64 */
            failed_items: number;
            finished_at?: string | null;
            id: string;
            repeat_request?: null | components["schemas"]["CreateTaskRequest"];
            root_name?: string | null;
            /** Format: int64 */
            skipped_items: number;
            source_label: string;
            status: components["schemas"]["TaskStatus"];
            task_id: string;
            /** Format: int64 */
            total_items: number;
        };
        DownloadSource: {
            query: string;
            /** @enum {string} */
            type: "query";
        } | {
            post_ids: number[];
            /** @enum {string} */
            type: "post_ids";
        };
        DownloadTaskRequest: {
            batch_filter?: null | components["schemas"]["BatchDownloadFilter"];
            /** Format: int32 */
            concurrency: number;
            filename_template: string;
            keep_sidecar_txt?: boolean | null;
            /** Format: int64 */
            limit: number;
            media_policy: components["schemas"]["MediaPolicy"];
            prioritize_resolution?: boolean | null;
            prioritize_score?: boolean | null;
            relative_directory?: string | null;
            root_id: string;
            skip_existing: boolean;
            source: components["schemas"]["DownloadSource"];
            static_images_only?: boolean | null;
            /**
             * @description discriminator enum property added by openapi-typescript
             * @enum {string}
             */
            type: "download";
        };
        /** @enum {string} */
        DownloadTaskType: "download";
        ExactDedupTaskRequest: {
            options?: null | components["schemas"]["PreflightTaskOptions"];
            root_id: string;
            /**
             * @description discriminator enum property added by openapi-typescript
             * @enum {string}
             */
            type: "exact_dedup";
        };
        /** @enum {string} */
        ExactDedupTaskType: "exact_dedup";
        /** @enum {string} */
        HealthState: "ok";
        HealthStatus: {
            database: components["schemas"]["DatabaseHealthState"];
            status: components["schemas"]["HealthState"];
            /** Format: int64 */
            uptime_seconds: number;
            version: string;
        };
        HeicConvertTaskRequest: {
            options: components["schemas"]["MediaIdsTaskOptions"];
            root_id: string;
            /**
             * @description discriminator enum property added by openapi-typescript
             * @enum {string}
             */
            type: "heic_convert";
        };
        /** @enum {string} */
        HeicConvertTaskType: "heic_convert";
        IndexLibraryTaskRequest: {
            root_id: string;
            /**
             * @description discriminator enum property added by openapi-typescript
             * @enum {string}
             */
            type: "index_library";
        };
        /** @enum {string} */
        IndexLibraryTaskType: "index_library";
        IntegrityScanTaskRequest: {
            options?: null | components["schemas"]["PreflightTaskOptions"];
            root_id: string;
            /**
             * @description discriminator enum property added by openapi-typescript
             * @enum {string}
             */
            type: "integrity_scan";
        };
        /** @enum {string} */
        IntegrityScanTaskType: "integrity_scan";
        LibraryFacets: {
            /** Format: int64 */
            catalog_revision: number;
            resolution_ranges: components["schemas"]["LibraryResolutionRange"][];
            score_ranges: components["schemas"]["LibraryScoreRange"][];
            /** Format: int64 */
            total: number;
        };
        LibraryPage: {
            /** Format: int64 */
            catalog_revision: number;
            items: components["schemas"]["LocalMedia"][];
            next_cursor?: string | null;
            /** Format: int64 */
            page: number;
            previous_cursor?: string | null;
            resolution_ranges: components["schemas"]["LibraryResolutionRange"][];
            score_ranges: components["schemas"]["LibraryScoreRange"][];
            /** Format: int64 */
            total: number;
            /** Format: int64 */
            total_pages: number;
        };
        LibraryResolutionRange: {
            /** Format: int64 */
            count: number;
            /** Format: int64 */
            resolution_max: number;
            /** Format: int64 */
            resolution_min: number;
        };
        LibraryScoreRange: {
            /** Format: int64 */
            count: number;
            /** Format: int64 */
            score_max: number;
            /** Format: int64 */
            score_min: number;
        };
        LocalMedia: {
            created_at: string;
            /** Format: double */
            duration?: number | null;
            filename: string;
            /** Format: int32 */
            height?: number | null;
            id: string;
            mime_type: string;
            post_created_at?: string | null;
            /** Format: int64 */
            post_id?: number | null;
            rating?: null | components["schemas"]["ContentRating"];
            relative_path: string;
            root_id: string;
            /** Format: int64 */
            size_bytes: number;
            tags: string[];
            /** Format: int32 */
            width?: number | null;
        };
        LoraSvdAnalysisFileRequest: {
            label?: string | null;
            path: string;
        };
        LoraSvdAnalysisRequest: {
            /** @description Currently only `auto` is accepted. */
            device: string;
            files: components["schemas"]["LoraSvdAnalysisFileRequest"][];
            runtime_profile_id: string;
        };
        LoraSvdAnalysisResult: {
            comparison?: null | components["schemas"]["LoraSvdComparison"];
            execution: components["schemas"]["LoraSvdExecution"];
            /** Format: int64 */
            expires_at: number;
            id: string;
            reports: components["schemas"]["LoraSvdModelReport"][];
        };
        LoraSvdComparison: {
            checkpoints: components["schemas"]["LoraSvdComparisonCheckpoint"][];
            comparable: boolean;
            reason: string;
        };
        LoraSvdComparisonCheckpoint: {
            effective_rank: components["schemas"]["LoraSvdThresholdRanks"];
            id: string;
            label: string;
            /** Format: double */
            rank_utilization: number;
            /** Format: int64 */
            step?: number | null;
            /** Format: double */
            tail_energy_20: number;
        };
        LoraSvdCoverage: {
            /** Format: int64 */
            analyzed_modules: number;
            /** Format: int64 */
            candidate_modules: number;
            /** Format: int64 */
            unsupported_modules: number;
        };
        LoraSvdExcludedModule: {
            id: string;
            reason: string;
        };
        LoraSvdExecution: {
            device: string;
            /** Format: int64 */
            duration_ms: number;
            fallback: boolean;
            reason: string;
            selection_reason?: string | null;
        };
        LoraSvdModelReport: {
            architecture: string;
            coverage: components["schemas"]["LoraSvdCoverage"];
            /** Format: double */
            current_rank_energy: number;
            effective_rank: components["schemas"]["LoraSvdThresholdRanks"];
            excluded: components["schemas"]["LoraSvdExcludedModule"][];
            /** Format: int64 */
            file_size_bytes: number;
            format: string;
            global_cumulative_energy: number[];
            /**
             * Format: int64
             * @description Full point count before the initial response is reduced for interactive rendering.
             */
            global_cumulative_energy_count?: number | null;
            global_singular_values: number[];
            /**
             * Format: int64
             * @description Full point count before the initial response is reduced for interactive rendering.
             */
            global_singular_values_count?: number | null;
            id: string;
            label: string;
            metadata: {
                [key: string]: string;
            };
            /** Format: int64 */
            modified_at: number;
            modules: components["schemas"]["LoraSvdModule"][];
            path: string;
            rank_distribution: components["schemas"]["LoraSvdRankDistribution"];
            sha256: string;
            /** Format: int64 */
            step?: number | null;
            /** @description Whether a standard LoRA factor-pair QR-SVD is mathematically applicable. */
            svd_applicable: boolean;
            /** Format: double */
            tail_energy_20: number;
            verdict: string;
            verdict_message: string;
        };
        LoraSvdModule: {
            /** Format: double */
            alpha: number;
            component: string;
            effective_rank: components["schemas"]["LoraSvdThresholdRanks"];
            /** Format: double */
            energy: number;
            flag?: string | null;
            id: string;
            /** Format: int64 */
            numerical_rank: number;
            /** Format: int64 */
            rank: number;
            /** Format: double */
            scale: number;
            /** @description Returned from the module detail/export endpoints; omitted from the initial summary. */
            singular_values?: number[] | null;
            /** Format: double */
            stable_rank: number;
            /** Format: double */
            tail_energy_20: number;
        };
        LoraSvdRankDistribution: {
            /** Format: int64 */
            maximum: number;
            /** Format: int64 */
            minimum: number;
            /** Format: int64 */
            modal: number;
            uniform: boolean;
        };
        LoraSvdThresholdRanks: {
            /** Format: int64 */
            energy_95: number;
            /** Format: int64 */
            energy_99: number;
            /** Format: int64 */
            energy_999: number;
        };
        MediaDirectory: {
            relative_path: string;
        };
        MediaDirectoryList: {
            directories: string[];
            truncated: boolean;
        };
        MediaIdsTaskOptions: {
            excluded_media_ids?: string[] | null;
            /** Format: int64 */
            library_post_created_from?: number | null;
            /** Format: int64 */
            library_post_created_to?: number | null;
            library_query?: string | null;
            library_relative_directory?: string | null;
            media_ids?: string[] | null;
            relative_directory?: string | null;
            vllm?: null | components["schemas"]["VllmRetagTaskOptions"];
        };
        MediaPolicy: {
            original: boolean;
            ugoira: components["schemas"]["UgoiraPolicy"];
        };
        MediaRoot: {
            created_at: string;
            id: string;
            indexed: boolean;
            linux_path?: string | null;
            media_count: number;
            name: string;
            updated_at: string;
            windows_path?: string | null;
        };
        NearDedupTaskOptions: {
            /** Format: int32 */
            distance?: number | null;
            preflight?: boolean | null;
            relative_directory?: string | null;
        };
        NearDedupTaskRequest: {
            options?: null | components["schemas"]["NearDedupTaskOptions"];
            root_id: string;
            /**
             * @description discriminator enum property added by openapi-typescript
             * @enum {string}
             */
            type: "near_dedup";
        };
        /** @enum {string} */
        NearDedupTaskType: "near_dedup";
        NearDuplicatePair: {
            /** Format: int32 */
            distance: number;
            left: string;
            right: string;
        };
        PreflightTaskOptions: {
            preflight?: boolean | null;
            relative_directory?: string | null;
        };
        PurgeResponse: {
            purged: number;
        };
        QuarantineEntry: {
            created_at: string;
            id: string;
            original_relative_path: string;
            quarantine_relative_path: string;
            reason: string;
            root_id: string;
            /** Format: int64 */
            size_bytes: number;
        };
        ReindexLibraryTaskRequest: {
            root_id: string;
            /**
             * @description discriminator enum property added by openapi-typescript
             * @enum {string}
             */
            type: "reindex_library";
        };
        /** @enum {string} */
        ReindexLibraryTaskType: "reindex_library";
        ResizeTaskOptions: {
            excluded_media_ids?: string[] | null;
            /** Format: int64 */
            library_post_created_from?: number | null;
            /** Format: int64 */
            library_post_created_to?: number | null;
            library_query?: string | null;
            library_relative_directory?: string | null;
            /** Format: int32 */
            max_size?: number | null;
            media_ids?: string[] | null;
            /** Format: int32 */
            quality?: number | null;
            relative_directory?: string | null;
        };
        ResizeTaskRequest: {
            options: components["schemas"]["ResizeTaskOptions"];
            root_id: string;
            /**
             * @description discriminator enum property added by openapi-typescript
             * @enum {string}
             */
            type: "resize";
        };
        /** @enum {string} */
        ResizeTaskType: "resize";
        RootRemoval: {
            id: string;
        };
        SaveMediaRootRequest: {
            linux_path?: string | null;
            name: string;
            windows_path?: string | null;
        };
        SchedulerResourceDiagnostics: {
            available: number;
            capacity: number;
        };
        SecretRequest: {
            secret: string;
        };
        SecretResponse: {
            configured: boolean;
            storage: components["schemas"]["SecretStorage"];
        };
        /** @enum {string} */
        SecretStorage: "system" | "session" | "none";
        SmartCropTaskOptions: {
            cowboy_shot?: boolean | null;
            enabled?: boolean | null;
            feet?: boolean | null;
            full_body_tight?: boolean | null;
            gpu_id?: string | null;
            lower_body?: boolean | null;
            /** Format: int32 */
            max_derived_per_family?: number | null;
            portrait?: boolean | null;
            quality_profile?: string | null;
            require_both_feet?: boolean | null;
            runtime_profile_id?: string | null;
            upper_body?: boolean | null;
        };
        SystemDiagnostics: {
            active_workers: number;
            database_pool: components["schemas"]["DatabasePoolDiagnostics"];
            queued_tasks: number;
            scheduler: {
                [key: string]: components["schemas"]["SchedulerResourceDiagnostics"];
            };
            thumbnail_cache: components["schemas"]["ThumbnailCacheDiagnostics"];
        };
        /** @enum {string} */
        TagCategory: "general" | "artist" | "copyright" | "character" | "meta" | "query";
        TagPipelineTaskOptions: {
            artist_prefix?: null | components["schemas"]["ArtistPrefix"];
            excluded_media_ids?: string[] | null;
            /** Format: int64 */
            library_post_created_from?: number | null;
            /** Format: int64 */
            library_post_created_to?: number | null;
            library_query?: string | null;
            library_relative_directory?: string | null;
            media_ids?: string[] | null;
            relative_directory?: string | null;
        };
        TagPipelineTaskRequest: {
            options: components["schemas"]["TagPipelineTaskOptions"];
            root_id: string;
            /**
             * @description discriminator enum property added by openapi-typescript
             * @enum {string}
             */
            type: "tag_pipeline";
        };
        /** @enum {string} */
        TagPipelineTaskType: "tag_pipeline";
        TagSuggestion: {
            category: components["schemas"]["TagCategory"];
            label: string;
            /** Format: int64 */
            post_count?: number | null;
            value: string;
        };
        TaskDetails: {
            item_counts: components["schemas"]["TaskItemCounts"];
            items: components["schemas"]["TaskItem"][];
            next_cursor?: string | null;
            result?: unknown;
            task: components["schemas"]["TaskSummary"];
        };
        TaskEvent: {
            event_type: components["schemas"]["TaskEventType"];
            /** Format: int64 */
            revision: number;
            /** Format: int64 */
            sequence: number;
            task: components["schemas"]["TaskSummary"];
            task_id: string;
        };
        /** @enum {string} */
        TaskEventType: "created" | "updated" | "deleted";
        TaskFailure: {
            code: string;
            item_id?: string | null;
            message: string;
            retryable: boolean;
        };
        TaskItem: {
            /** Format: int64 */
            attempts: number;
            error?: null | components["schemas"]["TaskFailure"];
            item_id: string;
            /** Format: int64 */
            post_id?: number | null;
            result?: unknown;
            status: components["schemas"]["TaskItemStatus"];
            updated_at: string;
        };
        TaskItemCounts: {
            /** Format: int64 */
            completed: number;
            /** Format: int64 */
            completed_bytes: number;
            /** Format: int64 */
            failed: number;
            /** Format: int64 */
            queued: number;
            /** Format: int64 */
            retryable_failed: number;
            /** Format: int64 */
            skipped: number;
            /** Format: int64 */
            total: number;
        };
        /** @enum {string} */
        TaskItemStatus: "queued" | "completed" | "skipped" | "failed";
        /** @enum {string} */
        TaskKind: "download" | "index_library" | "reindex_library" | "integrity_scan" | "exact_dedup" | "near_dedup" | "resize" | "heic_convert" | "delete_by_tag" | "delete_selected" | "tag_pipeline" | "vllm_tag" | "dataset_augmentation" | "training" | "runtime_install";
        TaskPreview: {
            candidates?: components["schemas"]["TaskPreviewCandidate"][] | null;
            message?: string | null;
            pairs?: components["schemas"]["NearDuplicatePair"][] | null;
            /** Format: int64 */
            pending?: number | null;
            type?: string | null;
        };
        TaskPreviewCandidate: {
            companion_paths?: string[] | null;
            reason: string;
            relative_path: string;
            sha256?: string | null;
            /** Format: int64 */
            size: number;
        };
        TaskProgress: {
            /** Format: int64 */
            bytes_downloaded: number;
            /** Format: int64 */
            completed: number;
            /** Format: int64 */
            eta_seconds?: number | null;
            /** Format: int64 */
            speed_bytes_per_sec: number;
            /** Format: int64 */
            total: number;
            /** Format: int64 */
            total_bytes?: number | null;
        };
        /** @enum {string} */
        TaskResourceClass: "network" | "io" | "cpu" | "gpu" | "maintenance";
        TaskScheduling: {
            blocking_task_ids: string[];
            /** Format: int64 */
            estimated_wait_seconds?: number | null;
            /** Format: int64 */
            queue_position?: number | null;
            resource_class: components["schemas"]["TaskResourceClass"];
            wait_reason?: string | null;
        };
        TaskSnapshot: {
            /** Format: int64 */
            last_event_id: number;
            tasks: components["schemas"]["TaskSummary"][];
        };
        /** @enum {string} */
        TaskStatus: "queued" | "running" | "pausing" | "paused" | "cancelling" | "awaiting_confirmation" | "completed" | "failed" | "cancelled";
        TaskSummary: {
            created_at: string;
            failures: components["schemas"]["TaskFailure"][];
            id: string;
            kind: components["schemas"]["TaskKind"];
            preview?: null | components["schemas"]["TaskPreview"];
            progress: components["schemas"]["TaskProgress"];
            /** Format: int64 */
            revision: number;
            scheduling: components["schemas"]["TaskScheduling"];
            stage: string;
            status: components["schemas"]["TaskStatus"];
            title: string;
            training?: null | components["schemas"]["TrainingTaskSummary"];
            updated_at: string;
        };
        ThumbnailCacheDiagnostics: {
            /** Format: int64 */
            bytes: number;
            /** Format: int64 */
            entries: number;
        };
        TrainingAdapterField: {
            advanced: boolean;
            choices: string[];
            default: unknown;
            group: string;
            help: string;
            key: string;
            kind: string;
            label: string;
            required: boolean;
        };
        TrainingAdapterGroup: {
            description: string;
            id: string;
            label: string;
        };
        TrainingAdapterResponse: {
            family: string;
            family_label: string;
            fields: components["schemas"]["TrainingAdapterField"][];
            groups: components["schemas"]["TrainingAdapterGroup"][];
            id: string;
            label: string;
            trainer: string;
            training_type: string;
            training_type_label: string;
            version: string;
        };
        TrainingArtifact: {
            id: string;
            kind: string;
            /** Format: int64 */
            modified_at: number;
            name: string;
            path: string;
            /** Format: int64 */
            size_bytes: number;
            /** Format: int64 */
            step?: number | null;
            url: string;
        };
        TrainingArtifactsResponse: {
            artifacts: components["schemas"]["TrainingArtifact"][];
        };
        TrainingAugmentationDiscoveryResponse: {
            source: components["schemas"]["TrainingGalleryDatasetResponse"];
            subsets: components["schemas"]["TrainingAugmentationSubsetResponse"][];
        };
        TrainingAugmentationSubsetResponse: {
            /** Format: int64 */
            caption_count: number;
            caption_extension: string;
            id: string;
            /** Format: int64 */
            image_count: number;
            label: string;
            relative_directory: string;
            /** Format: int32 */
            repeats: number;
            task_id: string;
        };
        TrainingCleanupPath: {
            /** Format: int64 */
            bytes: number;
            /** Format: int64 */
            file_count: number;
            kind: string;
            path: string;
            reason?: string | null;
        };
        TrainingCleanupPreviewResponse: {
            deletable: components["schemas"]["TrainingCleanupPath"][];
            retained: components["schemas"]["TrainingCleanupPath"][];
        };
        TrainingCleanupResponse: {
            deleted: components["schemas"]["TrainingCleanupPath"][];
            retained: components["schemas"]["TrainingCleanupPath"][];
            task_id: string;
        };
        TrainingGalleryDataset: {
            caption_extension?: string | null;
            relative_directory: string;
            /** Format: int32 */
            repeats: number;
            root_id: string;
        };
        TrainingGalleryDatasetResponse: {
            /** Format: int64 */
            caption_count: number;
            caption_extension: string;
            /** Format: int64 */
            effective_image_count: number;
            /** Format: int64 */
            image_count: number;
            image_dir: string;
            relative_directory: string;
            /** Format: int32 */
            repeats: number;
            root_id: string;
            root_name: string;
        };
        TrainingGpu: {
            external_processes: components["schemas"]["TrainingGpuExternalProcess"][];
            /** Format: int64 */
            fan_speed_percent?: number | null;
            /** Format: int64 */
            graphics_clock_mhz?: number | null;
            id: string;
            /** Format: int64 */
            memory_clock_mhz?: number | null;
            /** Format: int64 */
            memory_total_mib: number;
            /** Format: int64 */
            memory_used_mib: number;
            name: string;
            /** Format: double */
            power_draw_w?: number | null;
            /** Format: double */
            power_limit_w?: number | null;
            /** Format: int64 */
            temperature_c?: number | null;
            /** Format: int64 */
            utilization_percent: number;
        };
        TrainingGpuExternalProcess: {
            /** Format: int64 */
            memory_used_mib: number;
            /** Format: int64 */
            pid: number;
            process_name: string;
        };
        TrainingLogsResponse: {
            /** Format: int64 */
            cursor: number;
            text: string;
            truncated: boolean;
        };
        TrainingMetric: {
            series: string;
            /** Format: int64 */
            step: number;
            /** Format: int64 */
            timestamp: number;
            /** Format: double */
            value: number;
        };
        TrainingMetricSeriesSummary: {
            /** Format: int64 */
            count: number;
            first: components["schemas"]["TrainingMetric"];
            latest: components["schemas"]["TrainingMetric"];
            maximum: components["schemas"]["TrainingMetric"];
            minimum: components["schemas"]["TrainingMetric"];
            series: string;
        };
        TrainingMetricsOverviewResponse: {
            /** Format: int64 */
            cursor: number;
            series: components["schemas"]["TrainingMetricSeriesSummary"][];
        };
        TrainingMetricsResponse: {
            /** Format: int64 */
            cursor: number;
            metrics: components["schemas"]["TrainingMetric"][];
        };
        TrainingParameterSuggestion: {
            field: string;
            reason: string;
            value: unknown;
        };
        TrainingPathBrowserResponse: {
            current_path: string;
            directories: components["schemas"]["TrainingPathEntry"][];
            files: components["schemas"]["TrainingPathEntry"][];
            parent_path?: string | null;
        };
        TrainingPathEntry: {
            name: string;
            path: string;
        };
        TrainingPreflightCheck: {
            id: string;
            message: string;
            ok: boolean;
            recovery?: string | null;
            severity: string;
        };
        TrainingPreflightResponse: {
            checks: components["schemas"]["TrainingPreflightCheck"][];
            /** Format: int64 */
            effective_steps: number;
            /** Format: int64 */
            estimated_vram_mib: number;
            ready: boolean;
            suggestions: components["schemas"]["TrainingParameterSuggestion"][];
        };
        TrainingPresetExportResponse: {
            name: string;
            toml: string;
        };
        TrainingPresetImportRequest: {
            adapter_id?: string | null;
            gpu_ids?: string[] | null;
            name: string;
            runtime_profile_id?: string | null;
            toml: string;
        };
        TrainingPresetInput: {
            name: string;
            training: components["schemas"]["TrainingRunRequest"];
        };
        TrainingPresetResponse: {
            /** Format: int64 */
            created_at: number;
            id: string;
            name: string;
            training: components["schemas"]["TrainingRunRequest"];
            /** Format: int64 */
            updated_at: number;
            version_count: number;
        };
        TrainingPreviewRequest: {
            adapter_id: string;
            parameters: unknown;
        };
        TrainingPreviewResponse: {
            toml: string;
        };
        TrainingQueueEntry: {
            adapter_id: string;
            assigned_gpu_ids: string[];
            blocked_gpu_ids: string[];
            blocking_task_ids: string[];
            /** Format: int64 */
            estimated_wait_seconds?: number | null;
            gpu_ids: string[];
            /** Format: int64 */
            queue_position?: number | null;
            runtime_profile_id: string;
            status: string;
            task_id: string;
            wait_reason?: string | null;
        };
        TrainingQueueResponse: {
            entries: components["schemas"]["TrainingQueueEntry"][];
        };
        TrainingRunRequest: {
            adapter_id: string;
            gallery_dataset?: null | components["schemas"]["TrainingGalleryDataset"];
            gallery_datasets: components["schemas"]["TrainingGalleryDataset"][];
            gpu_ids: string[];
            parameters: unknown;
            runtime_profile_id: string;
            sample?: null | components["schemas"]["TrainingSampleSettings"];
        };
        TrainingRuntimeCheck: {
            detail: string;
            id: string;
            ok: boolean;
        };
        TrainingRuntimeDiagnostics: {
            checks: components["schemas"]["TrainingRuntimeCheck"][];
            profile: components["schemas"]["TrainingRuntimeProfile"];
        };
        TrainingRuntimeProfile: {
            id: string;
            installed: boolean;
            installing: boolean;
            kind: string;
            label: string;
            last_error?: string | null;
            managed: boolean;
            python_path: string;
            runtime_root: string;
        };
        /** @enum {string} */
        TrainingSamplePromptSource: "manual" | "dataset_captions";
        TrainingSampleSettings: {
            /** Format: int32 */
            dataset_caption_count: number;
            enabled: boolean;
            /** Format: int32 */
            every_n_epochs: number;
            /** Format: int32 */
            height: number;
            negative_prompt: string;
            prompt: string;
            prompt_source: components["schemas"]["TrainingSamplePromptSource"];
            /** Format: int32 */
            steps: number;
            /** Format: int32 */
            width: number;
        };
        TrainingTaskRequest: {
            root_id: string;
            training: components["schemas"]["TrainingRunRequest"];
            /**
             * @description discriminator enum property added by openapi-typescript
             * @enum {string}
             */
            type: "training";
        };
        TrainingTaskSummary: {
            adapter_id: string;
            gpu_ids: string[];
            model_path?: string | null;
            output_dir?: string | null;
            output_name?: string | null;
            runtime_profile_id: string;
            train_data_dir?: string | null;
        };
        /** @enum {string} */
        TrainingTaskType: "training";
        /** @enum {string} */
        UgoiraPolicy: "webm_and_zip" | "webm_only" | "zip_only";
        UpdateConfigRequest: {
            background_image: string;
            /** Format: int32 */
            background_opacity: number;
            blur_sensitive_media: boolean;
            danbooru_username: string;
            download_concurrency: number;
            filename_template: string;
            proxy_url?: string | null;
            ugoira_policy: components["schemas"]["UgoiraPolicy"];
            vllm_allowed_hosts: string[];
            vllm_base_url: string;
            vllm_concurrency: number;
            vllm_language: components["schemas"]["VllmLanguage"];
            vllm_max_length: number;
            vllm_max_tags: number;
            vllm_model: string;
            vllm_reference_existing: boolean;
            vllm_system_prompt: string;
            vllm_tag_mode: components["schemas"]["VllmTagMode"];
            vllm_verify_danbooru: boolean;
        };
        VisionCropRuntimeHealth: {
            gpu_id: string;
            gpu_name?: string | null;
            installing: boolean;
            last_error?: string | null;
            message: string;
            models_ready: boolean;
            providers: string[];
            python_path: string;
            ready: boolean;
            runtime_profile_id: string;
        };
        VllmHealthStatus: {
            available: boolean;
            message: string;
            models: string[];
        };
        /** @enum {string} */
        VllmLanguage: "zh" | "en" | "danbooru";
        VllmLoadStatus: {
            message: string;
            state: string;
        };
        VllmRetagTaskOptions: {
            base_url?: string | null;
            /** Format: int32 */
            concurrency?: number | null;
            language?: null | components["schemas"]["VllmLanguage"];
            /** Format: int32 */
            max_length?: number | null;
            model?: string | null;
            system_prompt?: string | null;
        };
        /** @enum {string} */
        VllmTagMode: "overwrite" | "append";
        VllmTagTaskRequest: {
            options: components["schemas"]["MediaIdsTaskOptions"];
            root_id: string;
            /**
             * @description discriminator enum property added by openapi-typescript
             * @enum {string}
             */
            type: "vllm_tag";
        };
        /** @enum {string} */
        VllmTagTaskType: "vllm_tag";
        VllmUnloadStatus: {
            message: string;
            state: string;
        };
    };
    responses: never;
    parameters: never;
    requestBodies: never;
    headers: never;
    pathItems: never;
}
export type $defs = Record<string, never>;
export interface operations {
    get_config: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_AppConfig"];
                };
            };
        };
    };
    update_config: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["UpdateConfigRequest"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_AppConfig"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    danbooru_autocomplete: {
        parameters: {
            query: {
                q: string;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_Vec_TagSuggestion"];
                };
            };
            502: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    danbooru_count: {
        parameters: {
            query: {
                q: string;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_DanbooruCount"];
                };
            };
            502: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    danbooru_posts: {
        parameters: {
            query: {
                q: string;
                page?: string;
                limit?: number;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_DanbooruPostsPage"];
                };
            };
            502: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    danbooru_post: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_DanbooruPost"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    danbooru_media: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: number;
                variant: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/octet-stream": unknown;
                };
            };
            206: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/octet-stream": unknown;
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    diagnostics: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_SystemDiagnostics"];
                };
            };
        };
    };
    download_history: {
        parameters: {
            query?: {
                cursor?: string;
                limit?: number;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_DownloadHistoryPage"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    health: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_HealthStatus"];
                };
            };
            503: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    library_facets: {
        parameters: {
            query: {
                root_id: string;
                q?: string;
                score_min?: number;
                score_max?: number;
                resolution_min?: number;
                resolution_max?: number;
                post_created_from?: number;
                post_created_to?: number;
                directory?: string;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_LibraryFacets"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    list_library_items: {
        parameters: {
            query: {
                root_id: string;
                q?: string;
                cursor?: string;
                page?: number;
                score_min?: number;
                score_max?: number;
                min_resolution?: number;
                resolution_min?: number;
                resolution_max?: number;
                post_created_from?: number;
                post_created_to?: number;
                directory?: string;
                limit?: number;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_LibraryPage"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    library_item: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_LocalMedia"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    library_media: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: string;
                variant: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/octet-stream": unknown;
                };
            };
            206: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/octet-stream": unknown;
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    list_quarantine: {
        parameters: {
            query: {
                root_id: string;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_Vec_QuarantineEntry"];
                };
            };
        };
    };
    purge_quarantine: {
        parameters: {
            query: {
                root_id: string;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_PurgeResponse"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    restore_quarantine: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_QuarantineEntry"];
                };
            };
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    list_roots: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_Vec_MediaRoot"];
                };
            };
        };
    };
    create_root: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["SaveMediaRootRequest"];
            };
        };
        responses: {
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_MediaRoot"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    update_root: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["SaveMediaRootRequest"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_MediaRoot"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    delete_root: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_RootRemoval"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    list_root_directories: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_MediaDirectoryList"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    create_root_directory: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["CreateMediaDirectoryRequest"];
            };
        };
        responses: {
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_MediaDirectory"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    put_secret: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                kind: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["SecretRequest"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_SecretResponse"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    delete_secret: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                kind: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_SecretResponse"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    list_tasks: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_TaskSnapshot"];
                };
            };
        };
    };
    create_task: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["CreateTaskRequest"];
            };
        };
        responses: {
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_TaskSummary"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    task_events: {
        parameters: {
            query?: {
                after?: number;
            };
            header?: {
                "Last-Event-ID"?: number | null;
            };
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/event-stream": string;
                };
            };
        };
    };
    task_detail: {
        parameters: {
            query?: {
                item_status?: components["schemas"]["TaskItemStatus"];
                item_cursor?: string;
                item_limit?: number;
            };
            header?: never;
            path: {
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_TaskDetails"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    task_action: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: string;
                action: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_TaskSummary"];
                };
            };
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    training_adapters: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_Vec_TrainingAdapterResponse"];
                };
            };
        };
    };
    training_gallery_augmentations: {
        parameters: {
            query: {
                root_id: string;
                relative_directory?: string;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_TrainingAugmentationDiscoveryResponse"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    training_gallery_dataset: {
        parameters: {
            query: {
                root_id: string;
                relative_directory?: string;
                repeats: number;
                caption_extension?: string;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_TrainingGalleryDatasetResponse"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    training_gpus: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_Vec_TrainingGpu"];
                };
            };
        };
    };
    create_lora_svd_analysis: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["LoraSvdAnalysisRequest"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_LoraSvdAnalysisResult"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    export_lora_svd_analysis: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_LoraSvdAnalysisResult"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    lora_svd_module: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: string;
                module_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_LoraSvdModule"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    training_path_browser: {
        parameters: {
            query: {
                kind: string;
                path?: string;
            };
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_TrainingPathBrowserResponse"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    training_preflight: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["TrainingRunRequest"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_TrainingPreflightResponse"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    list_training_presets: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_Vec_TrainingPresetResponse"];
                };
            };
        };
    };
    create_training_preset: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["TrainingPresetInput"];
            };
        };
        responses: {
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_TrainingPresetResponse"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    import_training_preset: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["TrainingPresetImportRequest"];
            };
        };
        responses: {
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_TrainingPresetResponse"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    update_training_preset: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["TrainingPresetInput"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_TrainingPresetResponse"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    export_training_preset: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_TrainingPresetExportResponse"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    update_training_preset_from_toml: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["TrainingPresetImportRequest"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_TrainingPresetResponse"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    training_preview: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["TrainingPreviewRequest"];
            };
        };
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_TrainingPreviewResponse"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    training_queue: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_TrainingQueueResponse"];
                };
            };
        };
    };
    training_runtime_profiles: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_Vec_TrainingRuntimeProfile"];
                };
            };
        };
    };
    training_runtime_diagnostics: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_TrainingRuntimeDiagnostics"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    install_training_runtime: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_TrainingRuntimeProfile"];
                };
            };
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    delete_training_task: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_TrainingCleanupResponse"];
                };
            };
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    training_artifacts: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_TrainingArtifactsResponse"];
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    training_artifact_file: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: string;
                artifact_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/octet-stream": unknown;
                };
            };
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    training_cleanup_preview: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_TrainingCleanupPreviewResponse"];
                };
            };
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    training_events: {
        parameters: {
            query?: {
                after?: number;
            };
            header?: {
                "Last-Event-ID"?: number | null;
            };
            path: {
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "text/event-stream": string;
                };
            };
        };
    };
    training_logs: {
        parameters: {
            query?: {
                tail?: number;
                after?: number;
                limit?: number;
            };
            header?: never;
            path: {
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_TrainingLogsResponse"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    training_metrics: {
        parameters: {
            query: {
                series: string[];
                max_points?: number;
                from_step?: number;
                to_step?: number;
                from_timestamp?: number;
                to_timestamp?: number;
            };
            header?: never;
            path: {
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_TrainingMetricsResponse"];
                };
            };
        };
    };
    training_metrics_overview: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_TrainingMetricsOverviewResponse"];
                };
            };
        };
    };
    vision_crop_runtime_health: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_VisionCropRuntimeHealth"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    install_vision_crop_runtime: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_VisionCropRuntimeHealth"];
                };
            };
            409: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    vllm_health: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_VllmHealthStatus"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    vllm_load: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_VllmLoadStatus"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
            500: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
    vllm_unload: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiSuccess_VllmUnloadStatus"];
                };
            };
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
            500: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiFailure"];
                };
            };
        };
    };
}
