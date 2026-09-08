<script lang="ts">
    import { locale, setLocale, tr } from '../../../lib/i18n';
    import ChoiceField from './SettingsChoiceField.svelte';
</script>

<div class="settings-workspace">
    <section class="settings-purpose-card settings-form">
        <ChoiceField
            id="app-language"
            label={$tr('settingsUi.screenLanguage')}
            value={$locale}
            options={[
                { value: 'ko', label: $tr('settingsUi.korean') },
                { value: 'en', label: $tr('settingsUi.english') },
            ]}
            onSelect={(value: string) => {
                if (value === 'ko' || value === 'en') setLocale(value);
            }}
        />
        <p class="settings-note">{$tr('settingsLive.languageHint')}</p>
    </section>
    <section class="settings-purpose-card settings-form">
        <h2>{$tr('settingsUi.replyLanguage')}</h2>
        <p class="settings-note">{$tr('settingsUi.replyLanguageHint')}</p>
    </section>
</div>
