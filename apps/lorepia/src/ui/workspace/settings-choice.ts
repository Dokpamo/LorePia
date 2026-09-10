export interface ChoiceRequest {
    label: string;
    value: string;
    options: { value: string; label: string; description?: string }[];
    onselect: (value: string) => void;
    action?: { label: string; run: () => void };
}
