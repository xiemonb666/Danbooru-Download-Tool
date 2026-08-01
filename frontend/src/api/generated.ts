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
        ApiSuccess_LibraryPage: {
            data: {
                items: components["schemas"]["LocalMedia"][];
                next_cursor?: string | null;
                /** Format: int64 */
                total: number;
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
                status: components["schemas"]["TaskStatus"];
                title: string;
                updated_at: string;
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
        AppConfig: {
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
        /** @enum {string} */
        ContentRating: "g" | "s" | "q" | "e" | "unknown";
        CreateMediaDirectoryRequest: {
            relative_path: string;
        };
        CreateTaskRequest: components["schemas"]["DownloadTaskRequest"] | components["schemas"]["IndexLibraryTaskRequest"] | components["schemas"]["IntegrityScanTaskRequest"] | components["schemas"]["ExactDedupTaskRequest"] | components["schemas"]["NearDedupTaskRequest"] | components["schemas"]["ResizeTaskRequest"] | components["schemas"]["HeicConvertTaskRequest"] | components["schemas"]["DeleteByTagTaskRequest"] | components["schemas"]["TagPipelineTaskRequest"] | components["schemas"]["VllmTagTaskRequest"];
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
        LibraryPage: {
            items: components["schemas"]["LocalMedia"][];
            next_cursor?: string | null;
            /** Format: int64 */
            total: number;
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
        MediaDirectory: {
            relative_path: string;
        };
        MediaDirectoryList: {
            directories: string[];
            truncated: boolean;
        };
        MediaIdsTaskOptions: {
            media_ids?: string[] | null;
            relative_directory?: string | null;
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
        ResizeTaskOptions: {
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
        SecretRequest: {
            secret: string;
        };
        SecretResponse: {
            configured: boolean;
            storage: components["schemas"]["SecretStorage"];
        };
        /** @enum {string} */
        SecretStorage: "system" | "session" | "none";
        /** @enum {string} */
        TagCategory: "general" | "artist" | "copyright" | "character" | "meta" | "query";
        TagPipelineTaskOptions: {
            artist_prefix?: null | components["schemas"]["ArtistPrefix"];
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
        TaskEventType: "created" | "updated";
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
        TaskKind: "download" | "index_library" | "integrity_scan" | "exact_dedup" | "near_dedup" | "resize" | "heic_convert" | "delete_by_tag" | "tag_pipeline" | "vllm_tag";
        TaskPreview: {
            candidates?: components["schemas"]["TaskPreviewCandidate"][] | null;
            pairs?: components["schemas"]["NearDuplicatePair"][] | null;
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
            status: components["schemas"]["TaskStatus"];
            title: string;
            updated_at: string;
        };
        /** @enum {string} */
        UgoiraPolicy: "webm_and_zip" | "webm_only" | "zip_only";
        UpdateConfigRequest: {
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
        VllmHealthStatus: {
            available: boolean;
            message: string;
            models: string[];
        };
        /** @enum {string} */
        VllmLanguage: "zh" | "en" | "danbooru";
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
    list_library_items: {
        parameters: {
            query: {
                root_id: string;
                q?: string;
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
}
