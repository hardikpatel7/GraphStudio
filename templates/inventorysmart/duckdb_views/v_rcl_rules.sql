-- RCL rule registry — joins the three raw_rcl_psm_* tables to surface
-- (rcl_code, rule_code, dim_json, priority, eligible_psa_count). 109 rows
-- on bealls. Backs dv_rcl_rules.
CREATE OR REPLACE VIEW v_rcl_rules AS
SELECT
  d.rcl_code,
  d.rule_code,
  d.dim_json,
  COALESCE(p.priority, -1)        AS priority,
  COUNT(DISTINCT e.psa_code)      AS eligible_psa_count
FROM raw_rcl_psm_rule_dim AS d
LEFT JOIN raw_rcl_psm_priorities  AS p ON p.rcl_code = d.rcl_code
LEFT JOIN raw_rcl_psm_eligibility AS e
       ON e.rcl_code = d.rcl_code AND e.rule_code = d.rule_code
GROUP BY d.rcl_code, d.rule_code, d.dim_json, p.priority;
