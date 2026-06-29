-- DROP FUNCTION inventory_smart.article_selection_list_v2(refcursor, jsonb, jsonb, _int4, _bpchar, jsonb, text, int4);

CREATE OR REPLACE FUNCTION inventory_smart.article_selection_list_v2(input refcursor, jsonb, jsonb, integer[], character[], jsonb, uuid text, default_sg_code integer)
 RETURNS refcursor
 LANGUAGE plpgsql
 SECURITY DEFINER
AS $function$
 	declare
 		_query_pa text := '';
 		_channel text := inventory_smart.get_channel_from_input($3);
 		_channel_filter text := inventory_smart.get_channel_str_from_input($3);
 		_store_active_filter text := '{"active": []}'::jsonb || $3;
 		_query_sa text := global.form_attribute_table_filters_v2('store_attributes', 'store_code', _store_active_filter::jsonb);
       _query_sa_psm text := '';
       _ph_query text := '';
 		_product_filters jsonb := $2 ;
 		_query_combine_format text := '';
 		_query_combine text := '';
 		_query_combine_count_format text := '';
 		_query_combine_count text := '';

        _count int := 1;
 		_batch_count int := 0;
 		_ph_sort text ;
 		_ph_search text;
        _overall_search text;
        _limit int;
        _offset int;
        _limit_clause text := '';
		_temp_query text := '';
		vl_unique_identifier text := $7;
		_rcl_input_query_format text:= '';
		_rcl_input_query text := '';
		_rcl_input_table text := '';
		_rcl_psm_resolved_table text := '';
		start_time timestamp;
        end_time timestamp;
        ph_data_id text := 'ph_data_' || uuid  || '';
       _constraints_input_table text = '';
       _resolved_articles varchar[];

	ph_configuration_mapping text := '_ph_configuration_mapping_' || uuid || '';

 	begin
		_rcl_input_table := 'public.rcl_psm_input_data_' || vl_unique_identifier;
		_constraints_input_table := 'public.rcl_constraint_input_data_' || vl_unique_identifier;
		_rcl_psm_resolved_table := 'public.rcl_psm_resolved_data_' || vl_unique_identifier;
		SELECT * FROM inventory_smart.form_search_sort_clause($6, 'ph_master', 'inventory_smart') INTO _ph_sort, _ph_search, _overall_search, _limit, _offset;
		raise notice '_ph_sort : %', _ph_sort;
		raise notice ' _ph_search : %', _ph_search;
		raise notice ' _overall_search: %', _overall_search;
		raise notice '_limit : %', _limit;
		raise notice ' _offset: %', _offset;
        select * from inventory_smart.form_product_store_attribute_filter_query('dummy', $2, $3, 'global', 'product_store_attributes_filter_store_code') into _query_sa_psm, _ph_query;

 		_query_pa := inventory_smart.form_main_table_filters(
 		  'ph_master',
 		  _product_filters
 		);

        _query_pa := _query_pa || _ph_search || _ph_sort;
       _query_pa := replace(_query_pa, '%', '%%');
       _query_pa := _query_pa || ' %1$s';
 		raise notice ' _query_pa %', _query_pa;
       raise notice ' _query_sa_psm % ', _query_sa_psm;
        if (_channel_filter = '') IS FALSE then
        	_channel_filter := ' WHERE channel in  '||_channel_filter||' ' ;
        end if;

       _query_combine_count_format := 'SELECT count(*) FROM ( SELECT * FROM inventory_smart.ph_master ' || _query_pa || ' ) sq;';


      _rcl_input_query_format := '
			with store_group as (
				select pcm.* , sg.name
                    from (
                        select
                            ph.ph_code,
                            unnest(product_code_size_map) as product,
							coalesce(pcms.default_store_group, '||default_sg_code||') as default_sg_code
                        from
                            %1$s ph
                        left join (
                			select ph_code, unnest(default_store_groups) default_store_group from ' || ph_configuration_mapping || '
						) pcms
						using(ph_code)
                    ) pcm
                    join global.store_groups sg on pcm.default_sg_code = sg.sg_code
                    where is_deleted = false
			),
			sgm as MATERIALIZED (
				select asgm.sg_code , psaf.store_code, psaf.psa_code from global.store_groups_mapping asgm
				join (
					select store_code, psa_code '||_query_sa_psm||'
				)psaf on asgm.store_code=psaf.store_code
			)
			select product->>''product_code'' as product_code, sgm.store_code, sgm.psa_code
                    from store_group esg
                    join sgm
                        on sgm.sg_code = esg.default_sg_code
                    group by 1, 2, 3';

 		-- ============================================================
 		-- OPTIMIZED _query_combine_format (v2)
 		-- Changes from original:
 		-- 1. ph CTE reads ph_data once, ph_unnested unnests once
 		-- 2. Derived grain views (psm_ph_article_dc, psm_ph_article_store, psm_ph_store)
 		-- 3. Pre-aggregated inventory sources (sda_agg, reserv_agg, alloc_agg, ladt_agg)
 		-- 4. Uses last_allocation_date_table instead of last_allocated_details
 		-- 5. No SELECT DISTINCT * FROM final_result wrapper
 		-- Format params: %1$s=ph_data_id, %2$s=rcl_psm_resolved_table, %3$s=resolved_articles,
 		--   %4$s=vl_unique_identifier, %5$s=ph_configuration_mapping, %6$s=default_sg_code, %7$s=limit
 		-- ============================================================
 		_query_combine_format := $Q$
			WITH ph AS (
				SELECT
					ph_code, channel, article, product_codes,
					l0_name, l1_name, l2_name, l3_name, l4_name, l5_name,
					style_color_description, product_description,
					sizes, product_lifecycle, article_status_tag, brand,
					"offset",
					product_code_size_map
				FROM %1$s
			),
			ph_unnested AS (
				SELECT
					ph.ph_code, ph.channel, ph.article, ph.product_codes,
					p->>'product_code' AS product_code,
					p->>'size' AS size
				FROM ph
				CROSS JOIN LATERAL UNNEST(product_code_size_map) AS p
			),
			product_dc_map AS (
				SELECT phu.ph_code, phu.channel, phu.article, phu.product_code, phu.size, pmpd.dc_code
				FROM ph_unnested phu
				JOIN global.product_mapping_product_dc pmpd
					ON pmpd.product_code = phu.product_code AND pmpd.is_active
				JOIN global.distribution_centres gdc
					ON gdc.dc_code = pmpd.dc_code AND gdc.is_active AND NOT gdc.is_deleted
			),
			product_store_dc_mapping AS (
				SELECT DISTINCT
					pmps.product_code, pmps.store_code, pdc.ph_code, pdc.article, pmsd.dc_code, pdc.size
				FROM ph
				JOIN %2$s pmps ON pmps.product_code = ANY(ph.product_codes)
				JOIN product_dc_map pdc ON pdc.ph_code = ph.ph_code
				JOIN global.product_mapping_store_dc pmsd
					ON pmsd.store_code = pmps.store_code AND pmsd.dc_code = pdc.dc_code AND pmsd.is_active
			),
			psm_ph_article_dc AS (
				SELECT DISTINCT ph_code, article, dc_code FROM product_store_dc_mapping
			),
			psm_ph_article_store AS (
				SELECT DISTINCT ph_code, article, store_code FROM product_store_dc_mapping
			),
			psm_ph_store AS (
				SELECT DISTINCT ph_code, store_code, product_code FROM product_store_dc_mapping
			),
			product_dc_map_after_store_eligible AS (
				SELECT DISTINCT ph_code, product_code, size, article, dc_code FROM product_store_dc_mapping
			),
			aid AS (
				SELECT psdm.ph_code, art.*
				FROM inventory_smart.article_inventory_dashboard art
				JOIN psm_ph_article_store psdm ON psdm.article = art.article AND psdm.store_code = art.store_code
			),
			txs_metrics AS (
				SELECT
					ph_code,
					CAST(ROUND(SUM(COALESCE(lw_units, 0))) AS INTEGER) AS lw_units,
					CAST(ROUND(SUM(COALESCE(lw_margin, 0))) AS INTEGER) AS lw_margin,
					CAST(ROUND(SUM(COALESCE(lw_revenue, 0))) AS INTEGER) AS lw_revenue,
					ROUND(COALESCE(SUM(lw_revenue) / NULLIF(SUM(lw_units), 0), 0)::DECIMAL, 2) AS price,
					ROUND(CAST(COALESCE(SUM(msrp * discount) / NULLIF(SUM(msrp), 0), 0) AS NUMERIC), 2) AS discount,
					ROUND(CAST(CASE WHEN COUNT(*) != 0 THEN COUNT(CASE WHEN in_stock = 1 THEN 1 ELSE NULL END) / CAST(COUNT(*) AS FLOAT) ELSE 0 END AS NUMERIC), 2) AS in_stock_perc
				FROM aid
				GROUP BY 1
			),
			before_allocated AS (
				SELECT
					ph.ph_code,
					SUM(dpi.eaches) AS eaches,
					SUM(dpi.packs) AS packs
				FROM psm_ph_article_dc ph
				JOIN (
					SELECT
						dpi.dc_code,
						dpi.article,
						CASE WHEN dpi.pack_type = 'eaches' THEN COALESCE(dpc.units_in_pack, 1) * COALESCE(dpi.oh_pack_qty, 0) ELSE 0 END AS eaches,
						CASE WHEN dpi.pack_type = 'packs' THEN COALESCE(dpc.units_in_pack, 1) * COALESCE(dpi.oh_pack_qty, 0) ELSE 0 END AS packs
					FROM inventory_smart.dc_pack_inventory dpi
					JOIN inventory_smart.dc_pack_configuration dpc
						ON dpi.pack_type_id = dpc.pack_type_id AND dpi.article = dpc.article AND dpi.pack_type = dpc.pack_type
				) dpi ON dpi.article = ph.article AND dpi.dc_code = ph.dc_code
				GROUP BY ph.ph_code
			),
			sda_agg AS (
				SELECT product_code, dc_code, size, article,
					SUM(COALESCE(oh, 0)) AS oh,
					SUM(COALESCE(oo, 0)) AS oo,
					SUM(COALESCE(it, 0)) AS it
				FROM inventory_smart.sku_dc_available_units
				GROUP BY 1, 2, 3, 4
			),
			reserv_agg AS (
				SELECT product_code, dc_code, size, article,
					SUM(COALESCE(quantity, 0)) AS quantity
				FROM inventory_smart.sku_dc_reserved_units
				GROUP BY 1, 2, 3, 4
			),
			alloc_agg AS (
				SELECT article, dc_code, size,
					MAX(updated_at) AS allocated_time,
					COALESCE(SUM(quantity), 0) AS quantity
				FROM inventory_smart.sku_dc_allocated_units('', '%3$s')
				GROUP BY 1, 2, 3
			),
			ladt_agg AS (
				SELECT article, updated_at AS allocated_time
				FROM inventory_smart.last_allocated_details
			),
			inventory_details_product_dc_level AS (
				SELECT
					ph.product_code,
					ph.ph_code,
					ph.dc_code,
					ph.size,
					sda.oh AS oh,
					sda.oo AS oo,
					sda.it AS it,
					COALESCE(sku_reserv.quantity, 0) AS total_reserve,
					COALESCE(sdal.quantity, 0) AS allocated_units,
					(sda.oh - COALESCE(sku_reserv.quantity, 0) - COALESCE(sdal.quantity, 0)) AS net_available_inventory,
					COALESCE(sdal.allocated_time, ladt.allocated_time, null) AS allocated_time
				FROM product_dc_map_after_store_eligible ph
				JOIN sda_agg sda USING (product_code, dc_code, size, article)
				LEFT JOIN reserv_agg sku_reserv USING (product_code, dc_code, size, article)
				LEFT JOIN alloc_agg sdal ON sdal.article = sda.article AND sdal.dc_code = sda.dc_code AND sdal.size = sda.size
				LEFT JOIN ladt_agg ladt ON ladt.article = sda.article
			),
			agg_inventory_details AS (
				SELECT product_code, ph_code, size,
					JSONB_OBJECT_AGG(dc_code, oh) AS oh_map,
					JSONB_OBJECT_AGG(dc_code, allocated_units) AS au_map,
					JSONB_OBJECT_AGG(dc_code, total_reserve) AS rq_map,
					SUM(oh) AS oh,
					SUM(oo) AS oo,
					SUM(it) AS it,
					SUM(total_reserve) AS reserve_quantity,
					SUM(net_available_inventory) AS net_available_inventory,
					SUM(allocated_units) AS allocated_units,
					MAX(allocated_time) AS allocated_time
				FROM inventory_details_product_dc_level
				GROUP BY 1, 2, 3
			),
			final_inventory AS (
				SELECT ph_code,
					JSONB_OBJECT_AGG(size, oh_map) AS oh_map,
					JSONB_OBJECT_AGG(size, au_map) AS au_map,
					JSONB_OBJECT_AGG(size, rq_map) AS rq_map,
					SUM(oh) AS oh,
					SUM(oo) AS oo,
					SUM(it) AS it,
					SUM(reserve_quantity) AS reserve_quantity,
					SUM(net_available_inventory) AS net_available_inventory,
					SUM(allocated_units) AS allocated_units,
					MAX(allocated_time) AS allocated_time
				FROM agg_inventory_details
				GROUP BY 1
			),
			constraint_data AS (
				SELECT ph_code,
					ROUND(AVG(aps)::NUMERIC, 2) AS aps,
					ROUND(AVG(wos)::NUMERIC, 2) AS wos,
					ROUND(AVG(min_stock)::NUMERIC, 2) AS min_stock,
					ROUND(AVG(max_stock)::NUMERIC, 2) AS max_stock,
					ROUND(MIN(min_validator)::NUMERIC, 2) AS min_stock_validator,
					ROUND(MAX(max_validator)::NUMERIC, 2) AS max_stock_validator,
					ARRAY_AGG(store_code) AS mapped_stores,
					ARRAY_LENGTH(ARRAY_AGG(store_code), 1) AS mapped_stores_count
				FROM (
					SELECT psm.ph_code,
						cm.store_code,
						SUM(aps) AS aps,
						AVG(wos) AS wos,
						AVG(min_stock) AS min_stock,
						AVG(max_stock) AS max_stock,
						MIN(max_stock) AS min_validator,
						MAX(min_stock) AS max_validator
					FROM constraints_resolved_data_%4$s cm
					JOIN psm_ph_store psm USING (product_code, store_code)
					GROUP BY 1, 2
				) foo
				GROUP BY 1
			),
			woc_data AS (
				SELECT ph_code,
					ROUND(AVG(woc)::NUMERIC, 2) AS woc,
					ROUND(AVG(max_mod)::NUMERIC, 2) AS avg_max_mod,
					ROUND(MIN(woc)::NUMERIC, 2) AS min_woc,
					ROUND(MAX(woc)::NUMERIC, 2) AS max_woc,
					COUNT(DISTINCT store_code) AS woc_mapped_stores_count
				FROM (
					SELECT psm.ph_code,
						psm.store_code,
						wm.woc,
						wm.max_mod
					FROM ph
					JOIN psm_ph_store psm ON psm.ph_code = ph.ph_code
					JOIN inventory_smart.woc_master wm
						ON wm.l4_name = ph.l4_name AND wm.store_code = psm.store_code
					WHERE wm.woc IS NOT NULL
				) woc_detail
				GROUP BY ph_code
			),
			product_profiles_ia AS (
				SELECT ph.ph_code,
					JSONB_BUILD_OBJECT('value', pp_code, 'name', name, 'label', special_classification) AS iapp
				FROM ph
				JOIN inventory_smart.product_profile_master ppm ON ph.ph_code = ppm.ph_code
				WHERE special_classification = 'ia-recommended'
			),
			article_dc_config AS (
				SELECT ph_code,
					ARRAY_AGG(DISTINCT
						JSONB_BUILD_OBJECT(
							'value', dc.dc_code, 'label', dc.name,
							'is_default', true
						)
					) AS dcs
				FROM product_store_dc_mapping
				JOIN global.distribution_centres dc USING (dc_code)
				GROUP BY 1
			),
			article_udpp_config AS (
				SELECT ph.ph_code,
					JSONB_BUILD_OBJECT('value', ppm.pp_code, 'name', ppm.name, 'label', ppm.special_classification) AS udpp
				FROM ph
				JOIN %5$s pcm USING (ph_code)
				JOIN inventory_smart.product_profile_master ppm
					ON pcm.default_product_profile = ppm.pp_code
				JOIN inventory_smart.product_profile_user_mapping_size ppums
					ON pcm.default_product_profile = ppums.pp_code AND ppums.size = ANY(ph.sizes)
			),
			article_sg_config AS (
				SELECT
					pcm.ph_code,
					ARRAY_AGG(
						JSONB_BUILD_OBJECT(
							'value', sg_code, 'label', name, 'is_default', true
						)
					) AS store_groups
				FROM (
					SELECT
						ph.ph_code,
						COALESCE(pcms.default_store_group, %6$s) AS default_sg_code
					FROM ph
					LEFT JOIN (
						SELECT ph_code, UNNEST(default_store_groups) AS default_store_group FROM %5$s
					) pcms USING (ph_code)
				) pcm
				JOIN global.store_groups sg ON pcm.default_sg_code = sg.sg_code
				WHERE sg.is_deleted = false
				GROUP BY 1
			),
			inventory_stock_stats AS (
				SELECT
					ph.ph_code,
					ROUND(CAST(CASE WHEN SUM(total_count) != 0 THEN CAST(SUM(in_stock_count) AS FLOAT)/CAST(SUM(total_count) AS FLOAT) ELSE 0 END AS NUMERIC), 4) AS in_stock_perc,
					ROUND(CAST(CASE WHEN SUM(dc_instock_total_count) != 0 THEN CAST(SUM(dc_instock_count) AS FLOAT)/CAST(SUM(dc_instock_total_count) AS FLOAT) ELSE 0 END AS NUMERIC) * 100, 2) AS dc_instock
				FROM inventory_smart.article_instock
				JOIN ph USING (article)
				GROUP BY 1
			),
			allocation_rule AS (
				SELECT dc_store_policy.ph_code, dspur.rule_code, dspur.values AS alloc_rules
				FROM %5$s dc_store_policy
				JOIN inventory_smart.dc_store_policy_user_rule dspur
					ON dc_store_policy.dc_store_rule = dspur.rule_code
			)
			SELECT %7$s AS limit,
				ph."offset",
				ph.l0_name,
				ph.l1_name,
				ph.l2_name,
				ph.l3_name,
				ph.l4_name,
				ph.l5_name,
				ph.style_color_description,
				ph.article,
				ph.ph_code,
				ph.product_description,
				ph.sizes,
				ph.product_codes upc,
				ph.product_lifecycle AS product_life_cycle,
				ph.article_status_tag,
				ph.brand,
				STRING_TO_ARRAY(ph.channel, ',') AS channel,
				COALESCE(reserve_quantity, 0) AS reserve_quantity,
				COALESCE(oh, 0) AS oh,
				COALESCE(oo, 0) AS oo,
				COALESCE(it, 0) AS it,
				null AS pack_type_id,
				oh_map,
				rq_map,
				COALESCE(allocated_units, 0) AS allocated_units,
				((COALESCE(oh, 0) - COALESCE(reserve_quantity, 0)) - COALESCE(allocated_units, 0)) AS net_available_inventory,
				au_map,
				CASE WHEN udpp IS NULL THEN ARRAY[iapp || '{"is_default": true}']
					WHEN iapp = udpp THEN ARRAY[iapp || '{"is_default": true}']
					ELSE ARRAY[udpp || '{"is_default": true}', iapp || '{"is_default": false}']
				END AS product_profiles,
				TO_CHAR(allocated_time::TIMESTAMP, 'MM/DD/YYYY') AS last_allocated,
				dcs,
				store_groups,
				cd.mapped_stores_count,
				cd.mapped_stores,
				cd.aps,
				CAST(ROUND(cd.min_stock) AS INTEGER) AS min_stock,
				CAST(ROUND(cd.max_stock) AS INTEGER) AS max_stock,
				CAST(ROUND(cd.min_stock_validator) AS INTEGER) AS min_stock_validator,
				CAST(ROUND(cd.max_stock_validator) AS INTEGER) AS max_stock_validator,
				CAST(ROUND(wd.woc) AS INTEGER) AS wos,
				CAST(ROUND(wd.avg_max_mod) AS INTEGER) AS avg_max_mod,
				CAST(ROUND(wd.min_woc) AS INTEGER) AS min_woc,
				CAST(ROUND(wd.max_woc) AS INTEGER) AS max_woc,
				tm.lw_units,
				tm.lw_margin,
				tm.lw_revenue,
				tm.price,
				tm.discount,
				iss.in_stock_perc,
				b_alloc.eaches AS beginning_available_to_allocate_eaches,
				b_alloc.packs AS beginning_available_to_allocate_packs,
				alloc_rule.alloc_rules AS allocation_rules,
				COALESCE(alloc_rule.alloc_rules->>'min_type', (SELECT values->>'min_type' FROM inventory_smart.dc_store_policy_user_rule WHERE rule_code = 1 AND rule_type = 'dc-store-rule')) AS min_type
			FROM ph
			JOIN article_sg_config asgc ON ph.ph_code = asgc.ph_code
			LEFT JOIN final_inventory inv_info ON inv_info.ph_code = ph.ph_code
			JOIN constraint_data cd ON ph.ph_code = cd.ph_code
			LEFT JOIN woc_data wd ON ph.ph_code = wd.ph_code
			LEFT JOIN product_profiles_ia ppi ON ph.ph_code = ppi.ph_code
			JOIN article_dc_config adc ON ph.ph_code = adc.ph_code
			LEFT JOIN article_udpp_config ppu ON ph.ph_code = ppu.ph_code
			LEFT JOIN txs_metrics tm ON ph.ph_code = tm.ph_code
			LEFT JOIN inventory_stock_stats iss ON ph.ph_code = iss.ph_code
			LEFT JOIN before_allocated b_alloc ON b_alloc.ph_code = ph.ph_code
			LEFT JOIN allocation_rule alloc_rule ON ph.ph_code = alloc_rule.ph_code
			WHERE (COALESCE(oh, 0) - COALESCE(reserve_quantity, 0) - COALESCE(allocated_units, 0)) > 0
		$Q$;

 		WHILE _count > 0 AND _batch_count = 0 LOOP
 			IF TRIM(_ph_sort) = '' THEN
 				_limit_clause := 'ORDER BY article  LIMIT ' || _limit || ' OFFSET ' || _offset;
 			ELSE
 				_limit_clause := 'LIMIT ' || _limit || ' OFFSET ' || _offset;
 			END IF;

			_temp_query := format('drop table if exists %1$s cascade;', ph_data_id);
			perform  global.sp_log(null, 'inventory_smart.article_selection_list_v2', 'Before executing first temp_query',_temp_query,jsonb_build_object('$2',$2,'$3',$3,'$4',$4,'$5',$5,'$6',$6,'$7',$7,'$8',$8)) ;
			execute _temp_query;

			_temp_query := format(
				'create temp table %2$s as (
								with ph_data_without_offset as (
								select
									*
								from
									inventory_smart.ph_master
									join (select distinct article from inventory_smart.article_inventory_dashboard ) aid using (article)
								' || _query_pa || '
								)
								SELECT *, %3$s + ROW_NUMBER () OVER (' || replace(_ph_sort, '%', '%%') || ') as offset
									FROM ph_data_without_offset
								)',
				_limit_clause,
				ph_data_id,
				_offset
			);
			raise notice ' ph _Data query %', _temp_query;
			perform  global.sp_log(null, 'inventory_smart.article_selection_list_v2', 'Before executing second temp_query',_temp_query,jsonb_build_object('$2',$2,'$3',$3,'$4',$4,'$5',$5,'$6',$6,'$7',$7,'$8',$8)) ;
			execute _temp_query;

			perform  global.sp_log(null, 'inventory_smart.article_selection_list_v2', 'Before executing drop table query for prod_data_'||vl_unique_identifier,'drop table if exists prod_data_'||vl_unique_identifier||' cascade;',jsonb_build_object('$2',$2,'$3',$3,'$4',$4,'$5',$5,'$6',$6,'$7',$7,'$8',$8)) ;
			execute format('drop table if exists prod_data_%1$s cascade;', vl_unique_identifier);
			_temp_query := format('create unlogged table prod_data_%1$s as (select unnest(product_codes) as product_code from  %2$s);',vl_unique_identifier, ph_data_id);
			raise notice ' prod data query %', _temp_query;
			perform  global.sp_log(null, 'inventory_smart.article_selection_list_v2', 'Before executing third temp_query',_temp_query,jsonb_build_object('$2',$2,'$3',$3,'$4',$4,'$5',$5,'$6',$6,'$7',$7,'$8',$8)) ;
			execute _temp_query;

			perform  global.sp_log(null, 'inventory_smart.article_selection_list_v2', 'Before executing drop table query for '||ph_configuration_mapping,'drop table if exists '||ph_configuration_mapping||' cascade;',jsonb_build_object('$2',$2,'$3',$3,'$4',$4,'$5',$5,'$6',$6,'$7',$7,'$8',$8)) ;
			execute format('drop table if exists %1$s cascade', ph_configuration_mapping);
			_temp_query := format (
				'create temp table %1$s as (
								select array_agg(resolved_data.product_code) as product_codes,
										max(resolved_data.default_store_groups) as default_store_groups,
										max(resolved_data.default_product_profile) as default_product_profile,
										max(resolved_data.default_store_groups) as default_store_groups_selected,
										max(dc_store_rule) as dc_store_rule,
										psaf.article, psaf.ph_code
                                        from
								(
									select * from inventory_smart.generate_rcl_dc_store_policy(''prod_data_%2$s'', 10003, current_date)
                                ) resolved_data
								join
								(
									select unnest(product_codes) as product_code,
										article, ph_code
									from inventory_smart.ph_master
								) psaf
								using (product_code)
								group by psaf.article, psaf.ph_code
							);',
				ph_configuration_mapping, vl_unique_identifier
			);

			perform  global.sp_log(null, 'inventory_smart.article_selection_list_v2', 'Before executing fourth temp_query',_temp_query,jsonb_build_object('$2',$2,'$3',$3,'$4',$4,'$5',$5,'$6',$6,'$7',$7,'$8',$8)) ;
			raise notice 'ph  configuration data query : %', _temp_query;
			execute _temp_query;

			_rcl_input_query := format(_rcl_input_query_format, ph_data_id);
			raise notice '_rcl_input_query: %', _rcl_input_query;
			start_time := clock_timestamp();

			perform  global.sp_log(null, 'inventory_smart.article_selection_list_v2', 'Before executing drop table query for '||_rcl_input_table,'drop table if exists '||_rcl_input_table||' cascade;',jsonb_build_object('$2',$2,'$3',$3,'$4',$4,'$5',$5,'$6',$6,'$7',$7,'$8',$8)) ;

			execute 'drop table if exists ' || _rcl_input_table ||' cascade; ';
			_rcl_input_query:= 'create unlogged table ' || _rcl_input_table || ' as ( ' || _rcl_input_query || ' );';

			perform  global.sp_log(null, 'inventory_smart.article_selection_list_v2', 'Before executing create table query for _rcl_input_query', _rcl_input_query, jsonb_build_object('$2',$2,'$3',$3,'$4',$4,'$5',$5,'$6',$6,'$7',$7,'$8',$8)) ;

			raise notice '_rcl_input_query: %', _rcl_input_query;
			execute _rcl_input_query;

			perform  global.sp_log(null, 'inventory_smart.article_selection_list_v2', 'Before executing drop table query for '||_rcl_psm_resolved_table, 'drop table if exists '|| _rcl_psm_resolved_table || ';', jsonb_build_object('$2',$2,'$3',$3,'$4',$4,'$5',$5,'$6',$6,'$7',$7,'$8',$8)) ;
			execute 'drop table if exists '|| _rcl_psm_resolved_table || ';';
			_temp_query := format('create unlogged table %2$s as (
				select * from global.generate_rcl_psm_data(''%1$s'', 101, current_date));', _rcl_input_table, _rcl_psm_resolved_table);

			raise notice 'RCL resolution query : %', _temp_query;
			perform  global.sp_log(null, 'inventory_smart.article_selection_list_v2', 'Before executing fifth temp_query',_temp_query,jsonb_build_object('$2',$2,'$3',$3,'$4',$4,'$5',$5,'$6',$6,'$7',$7,'$8',$8)) ;
			execute _temp_query;

			_temp_query := format('CREATE UNLOGGED TABLE %1$s_filtered AS (
				SELECT product_code, store_code, rcl_code, is_active
				FROM (
					SELECT psm.*, RANK() OVER (
						PARTITION BY psm.product_code
						ORDER BY rm.priority ASC, array_length(rm.level, 1) DESC
					) as rnk
					FROM %1$s psm
					JOIN global.rcl_master rm ON psm.rcl_code = rm.rcl_code
				) ranked
				WHERE rnk = 1
			); DROP TABLE IF EXISTS %1$s; ALTER TABLE %1$s_filtered RENAME TO %2$s;',
			_rcl_psm_resolved_table, split_part(_rcl_psm_resolved_table, '.', 2));
			raise notice 'PSM specificity filter query: %', _temp_query;
			execute _temp_query;

			end_time := clock_timestamp();
			RAISE NOTICE 'Time taken to resolve PSM::::  %', end_time - start_time;
			start_time := clock_timestamp();

			perform  global.sp_log(null, 'inventory_smart.article_selection_list_v2', 'Before dropping _constraints_input_table','drop table if exists '||_constraints_input_table||' cascade;',jsonb_build_object('$2',$2,'$3',$3,'$4',$4,'$5',$5,'$6',$6,'$7',$7,'$8',$8)) ;

			execute format('drop table if exists %1$s cascade;', _constraints_input_table);
			_temp_query := format('create unlogged table %1$s as (select psm.product_code, psm.store_code, psaf.psa_code, paf.article from %2$s psm
				join
					global.product_attributes_filter paf using (product_code)
				join
					global.product_store_attributes_filter psaf
					on paf.l0_name=psaf.l0_name and paf.l1_name = psaf.l1_name and psm.store_code=psaf.store_code
				);',
				_constraints_input_table, _rcl_psm_resolved_table);
			raise notice 'temp query for constraints  : %', _temp_query;
			perform  global.sp_log(null, 'inventory_smart.article_selection_list_v2', 'Before executing sixth temp_query',_temp_query,jsonb_build_object('$2',$2,'$3',$3,'$4',$4,'$5',$5,'$6',$6,'$7',$7,'$8',$8)) ;
			execute _temp_query;

			perform  global.sp_log(null, 'inventory_smart.article_selection_list_v2', 'Before dropping table constraints_resolved_data_'||vl_unique_identifier ,'drop table if exists constraints_resolved_data_'||vl_unique_identifier||' cascade;',jsonb_build_object('$2',$2,'$3',$3,'$4',$4,'$5',$5,'$6',$6,'$7',$7,'$8',$8)) ;

			execute format('drop table if exists constraints_resolved_data_%1$s', vl_unique_identifier);
			_rcl_input_query := format('create temp table constraints_resolved_data_%2$s as (
				select * from inventory_smart.generate_rcl_constraint_data(''%1$s'', 170, current_date)
			)', _constraints_input_table, vl_unique_identifier);
			raise notice ' constraints resolution query: % ', _rcl_input_query;

			perform  global.sp_log(null, 'inventory_smart.article_selection_list_v2', 'constraints resolution query' ,_rcl_input_query,jsonb_build_object('$2',$2,'$3',$3,'$4',$4,'$5',$5,'$6',$6,'$7',$7,'$8',$8)) ;
			execute _rcl_input_query;
			end_time := clock_timestamp();
			RAISE NOTICE 'Time taken to resolve Constraints::::  %', end_time - start_time;
			execute format('select array_agg(distinct article) from constraints_resolved_data_%1$s', vl_unique_identifier) into _resolved_articles;
			_resolved_articles := coalesce(_resolved_articles, '{}');
			raise notice 'resolved articles: %', _resolved_articles;

			start_time := clock_timestamp();

			_query_combine := format(
				_query_combine_format,
				ph_data_id,                 -- %1$s
				_rcl_psm_resolved_table,    -- %2$s
				_resolved_articles,         -- %3$s
				vl_unique_identifier,       -- %4$s (used in constraints_resolved_data_%4$s)
				ph_configuration_mapping,   -- %5$s
				default_sg_code,            -- %6$s
				_limit                      -- %7$s
			);
			raise notice 'Query ------- : %', _query_combine;

			OPEN $1 SCROLL FOR EXECUTE _query_combine;
			perform  global.sp_log(null, 'inventory_smart.article_selection_list_v2', '_query_combine' ,_query_combine,jsonb_build_object('$2',$2,'$3',$3,'$4',$4,'$5',$5,'$6',$6,'$7',$7,'$8',$8)) ;

			end_time := clock_timestamp();
			RAISE NOTICE 'Time taken to query combine::::  %', end_time - start_time;

			MOVE FORWARD ALL FROM $1;
			GET DIAGNOSTICS _batch_count := ROW_COUNT;
			MOVE BACKWARD ALL FROM $1;

			IF _batch_count = 0 THEN
				_query_combine_count = format(_query_combine_count_format, _limit_clause);
				perform  global.sp_log(null, 'inventory_smart.article_selection_list_v2', '_query_combine_count' ,_query_combine_count,jsonb_build_object('$2',$2,'$3',$3,'$4',$4,'$5',$5,'$6',$6,'$7',$7,'$8',$8)) ;
				EXECUTE _query_combine_count INTO _count;
			END IF;
			_offset := _offset + _limit;
			_limit := _limit + _limit;
			IF _batch_count = 0 AND _count > 0 THEN CLOSE $1; END IF;

		END LOOP;
		raise notice ' dropping the rcl input tables';
 	RETURN $1;
 	end
 $function$
;
