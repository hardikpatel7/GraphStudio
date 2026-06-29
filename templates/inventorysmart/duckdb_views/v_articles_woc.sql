-- Per-article join of product hierarchy + WOC targets. asv2_woc.ph_code is
-- VARCHAR; asv2_ph_master.ph_code is BIGINT — cast on the join. Backs
-- dv_articles_over_woc.
CREATE OR REPLACE VIEW v_articles_woc AS
SELECT
  m.article, m.ph_code, m.l1_name, m.l2_name, m.l3_name, m.brand, m.channel,
  w.woc, w.min_woc, w.max_woc, w.avg_max_mod, w.woc_mapped_stores_count
FROM asv2_ph_master AS m
INNER JOIN asv2_woc AS w ON CAST(w.ph_code AS BIGINT) = m.ph_code;
