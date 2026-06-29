-- Store ↔ store_group bridge with the group name surfaced. Workaround for
-- the half-wired LevelSpec.key in graph_v2 (see fb_1778929220103740000):
-- both the spine and the bridge must join on `name`, so this view joins
-- the BIGINT sg_code through to expose the name on the mapping rows.
-- Backs the store_groups_mapping cross-edge on bealls-inventory-graph.
CREATE OR REPLACE VIEW v_store_groups_mapping AS
SELECT m.store_code, g.name
FROM raw_store_groups_mapping AS m
INNER JOIN raw_store_groups   AS g
        ON g.sg_code = m.sg_code AND g.is_deleted = FALSE;
