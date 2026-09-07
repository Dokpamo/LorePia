export interface ChoiceRequest {
    label: string;
    value: string;
    options: { value: string; label: string }[];
    onselect: (value: string) => void;
}
