export interface UiNoticeValue {
    id: number;
    text: string;
    retry?: () => void;
}
