PRAGMA foreign_keys = ON;

-- Legacy generations predate provider connections and therefore keep NULL
-- provenance. New catalog-target generations write both identifiers together.
ALTER TABLE generations
    ADD COLUMN model_route_id TEXT
        REFERENCES provider_models(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT;

ALTER TABLE generations
    ADD COLUMN generation_preset_id TEXT
        REFERENCES generation_presets(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT;

CREATE INDEX generations_model_route_started
    ON generations(model_route_id, started_at, id)
    WHERE model_route_id IS NOT NULL;

CREATE INDEX generations_preset_started
    ON generations(generation_preset_id, started_at, id)
    WHERE generation_preset_id IS NOT NULL;

CREATE TRIGGER generations_provider_target_insert_guard
BEFORE INSERT ON generations
WHEN
    (NEW.model_route_id IS NULL) <> (NEW.generation_preset_id IS NULL)
    OR (
        NEW.model_route_id IS NOT NULL
        AND NOT EXISTS (
            SELECT 1
            FROM generation_presets AS preset
            WHERE preset.id = NEW.generation_preset_id
              AND preset.model_route_id = NEW.model_route_id
        )
    )
BEGIN
    SELECT RAISE(ABORT, 'generation provider target is inconsistent');
END;

CREATE TRIGGER generations_provider_target_update_guard
BEFORE UPDATE OF model_route_id, generation_preset_id ON generations
WHEN
    (NEW.model_route_id IS NULL) <> (NEW.generation_preset_id IS NULL)
    OR (
        NEW.model_route_id IS NOT NULL
        AND NOT EXISTS (
            SELECT 1
            FROM generation_presets AS preset
            WHERE preset.id = NEW.generation_preset_id
              AND preset.model_route_id = NEW.model_route_id
        )
    )
BEGIN
    SELECT RAISE(ABORT, 'generation provider target is inconsistent');
END;
