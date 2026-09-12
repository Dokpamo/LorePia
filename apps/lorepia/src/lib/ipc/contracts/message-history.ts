import type { MessageDto } from './conversation';

export interface ListBranchMessagesPageInput {
    branch_id: string;
    before_message_id?: string | null;
    after_message_id?: string | null;
    limit: number;
    /** At most 256 override IDs whose membership should be checked in the same snapshot. */
    check_message_ids?: string[];
    /** Include the last assistant for card display when it falls outside the recent window. */
    include_last_assistant?: boolean;
}

/** Chronological messages and absolute positions in the returned head snapshot. */
export interface BranchMessagesPageDto {
    messages: MessageDto[];
    has_older: boolean;
    has_newer: boolean;
    head_message_id: string | null;
    total_messages: number;
    start_index: number;
    /** Present when membership was requested; preserves candidate order. */
    retained_message_ids?: string[];
    /** Supplemental last assistant only when absent from messages; omitted otherwise. */
    last_assistant_message?: MessageDto | null;
}
