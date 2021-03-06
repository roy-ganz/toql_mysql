//!
//! The Toql MySQL integration facade functions to load a struct from a MySQL database and insert, delete and update it.
//! The actual functionality is created by the Toql Derive that implements
//! the trait [Mutate](../toql/mutate/trait.Mutate.html).
//!


use mysql::{prelude::GenericConnection};

use crate::row::Row;

//use toql::mutate::collection_delta_sql;

use toql::key::Key;

use toql::key::Keyed;
use toql::page::Page;

use toql::query::{field_path::FieldPath, Query};

use toql::sql_mapper_registry::SqlMapperRegistry;

use toql::error::ToqlError;
use toql::sql_builder::SqlBuilder;

use core::borrow::Borrow;
use toql::alias::AliasFormat;
use toql::log_sql;

//use crate::row::FromResultRow;
use std::{
    borrow::BorrowMut,
    collections::{HashMap, HashSet}, sync::RwLockReadGuard,
};
use toql::fields::Fields;
use toql::paths::Paths;

//pub mod diff;
//pub mod insert;
//pub mod row;
//pub mod insert;
//pub mod update;

#[macro_use]
pub mod access;

//pub mod select;
pub use mysql; // Reexport for derive produced code

pub mod sql_arg;

pub mod error;
pub mod row;



use crate::error::Result;
use crate::error::ToqlMySqlError;
use toql::sql::Sql;
use toql::sql_arg::SqlArg;
use toql::tree::tree_predicate::TreePredicate;
use toql::tree::{
    tree_identity::TreeIdentity, tree_index::TreeIndex, tree_insert::TreeInsert,
    tree_keys::TreeKeys, tree_merge::TreeMerge, tree_update::TreeUpdate, tree_map::TreeMap,
};
use toql::{
    alias_translator::AliasTranslator,
    from_row::FromRow,
    parameter_map::ParameterMap,
    sql_expr::{resolver::Resolver, PredicateColumn},
    sql_mapper::{mapped::Mapped}, backend::context::Context, cache::Cache,
};

use crate::sql_arg::{values_from, values_from_ref};

fn load_count<T, B, C>(
    mysql: &mut MySql<C>,
    query: &B,
    page: Option<Page>,
) -> Result<Option<(u32, u32)>>
where
    T: Keyed
        + Mapped
        + FromRow<Row,ToqlMySqlError>
        + TreePredicate
        + TreeIndex<Row, ToqlMySqlError>
        + TreeMerge<Row, ToqlMySqlError>,
    B: Borrow<Query<T>>,
    <T as toql::key::Keyed>::Key: FromRow<Row,ToqlMySqlError>,
    C: GenericConnection,
    
{
    let page_count = if let Some(Page::Counted(_, _)) = page {
        let unpaged_count: u32 = {
            toql::log_literal_sql!("SELECT FOUND_ROWS();");
            let r = mysql.conn().query("SELECT FOUND_ROWS();")?;
            r.into_iter().next().unwrap().unwrap().get(0).unwrap()
        };
        let unfiltered_count: u32 = {
            
            let sql = {
                let alias_format = mysql.alias_format();
            let ty = <T as Mapped>::type_name();
            let mut alias_translator = AliasTranslator::new(alias_format);
            let aux_params = [mysql.aux_params()];
            let aux_params = ParameterMap::new(&aux_params);

            let registry= &*mysql.registry()?;
            let mut builder = SqlBuilder::new(&ty, registry);
            let result = builder.build_count("", query.borrow(), true)?;
                result
                .to_sql_with_modifier_and_extra(&aux_params, &mut alias_translator, "", "")
                .map_err(ToqlError::from)?
            };

            log_sql!(&sql);
            let Sql(sql_stmt, args) = sql;

            let args = crate::sql_arg::values_from_ref(&args);
            let query_results = mysql.conn.prep_exec(sql_stmt, args)?;
            query_results
                .into_iter()
                .next()
                .unwrap()
                .unwrap()
                .get(0)
                .unwrap()
        };
        Some((unpaged_count, unfiltered_count))
    } else {
        None
    };
    Ok(page_count)
}
fn load_top<T, B, C>(
    mysql: &mut MySql<C>,
    query: &B,
    page: Option<Page>,
) -> Result<(Vec<T>, HashSet<String>, Option<(u32, u32)>)>
where
    T: Keyed
        + Mapped
        + FromRow<Row,ToqlMySqlError>
        + TreePredicate
        + TreeIndex<Row, ToqlMySqlError>
        + TreeMerge<Row, ToqlMySqlError>,
    B: Borrow<Query<T>>,
    <T as toql::key::Keyed>::Key: FromRow<Row,ToqlMySqlError>,
    C: GenericConnection,
{
    use std::borrow::Cow;

    let alias_format = mysql.alias_format();

    let ty = <T as Mapped>::type_name();

   
    let result = {
        let registry =  &*mysql.registry()?;
        let mut builder = SqlBuilder::new(&ty,registry);
        builder.build_select("", query.borrow())?
    };

    let unmerged = result.unmerged_paths().clone();
    let mut alias_translator = AliasTranslator::new(alias_format);
    let aux_params = [mysql.aux_params()];
    let aux_params = ParameterMap::new(&aux_params);

    let extra = match page {
        Some(Page::Counted(start, number_of_records)) => {
            Cow::Owned(format!("LIMIT {},{}", start, number_of_records))
        }
        Some(Page::Uncounted(start, number_of_records)) => {
            Cow::Owned(format!("LIMIT {},{}", start, number_of_records))
        }
        None => Cow::Borrowed(""),
    };

    let modifier = if let Some(Page::Counted(_, _)) = page {
        "SQL_CALC_FOUND_ROWS"
    } else {
        ""
    };

    let sql = 
    {result
        .to_sql_with_modifier_and_extra(
            &aux_params,
            &mut alias_translator,
            modifier,
            extra.borrow(),
        )
        .map_err(ToqlError::from)?
        };

    log_sql!(&sql);
    let Sql(sql_stmt, args) = sql;

    let args = crate::sql_arg::values_from_ref(&args);
    let query_results = mysql.conn.prep_exec(sql_stmt, args)?;

    let mut entities: Vec<T> = Vec::new();
    for r in query_results {
        let r = Row(r?);
        let mut iter = result.selection_stream().iter();
        let mut i = 0usize;
       if let Some(e) = <T as toql::from_row::FromRow<Row,ToqlMySqlError>>::from_row(&r, &mut i, &mut iter)? 
       {
            entities.push(e);
        }
    
    }

    // Retrieve count information
    let page_count = load_count(mysql, query, page)?;

    Ok((entities, unmerged, page_count))
}

fn load_and_merge<T, B, C>(
    mysql: &mut MySql<C>,
    query: &B,
    entities: &mut Vec<T>,
    unmerged_paths: &HashSet<String>,
) -> Result<HashSet<String>>
where
    T: Keyed
        + Mapped
        + FromRow<Row,ToqlMySqlError>
        + TreePredicate
        + TreeIndex<Row, ToqlMySqlError>
        + TreeMerge<Row, ToqlMySqlError>,

    B: Borrow<Query<T>>,
    <T as toql::key::Keyed>::Key: FromRow<Row,ToqlMySqlError>,
    C: GenericConnection,
{
    use toql::sql_expr::SqlExpr;

    let ty = <T as Mapped>::type_name();
    let mut pending_paths = HashSet::new();

   
    let merge_base_alias = {
        let registry = &*mysql.registry()?;
        let mapper = registry
            .mappers
            .get(&ty)
            .ok_or(ToqlError::MapperMissing(ty.clone()))?;
         mapper.canonical_table_alias.clone()
    };

    for root_path in unmerged_paths {
        // Get merge JOIN with ON from mapper
       
        let mut result = {
            let registry = &*mysql.registry()?;
            let mut builder = SqlBuilder::new(&ty, registry); // Add alias format or translator to constructor
            builder.build_select(root_path.as_str(), query.borrow())?
        };
        pending_paths = result.unmerged_paths().clone();

        let other_alias = result.table_alias().clone();

        // Build merge join
        // Get merge join and custom on predicate from mapper
        let on_sql_expr = {
            let registry = &*mysql.registry()?;
            let builder = SqlBuilder::new(&ty, registry); // Add alias format or translator to constructor
            builder.merge_expr(&root_path)?
        };

        let (merge_join, merge_on) = {
            let merge_resolver = Resolver::new()
                .with_self_alias(&merge_base_alias)
                .with_other_alias(&result.table_alias());
            (
                merge_resolver
                    .resolve(&on_sql_expr.0)
                    .map_err(ToqlError::from)?,
                merge_resolver
                    .resolve(&on_sql_expr.1)
                    .map_err(ToqlError::from)?,
            )
        };

        //println!("{} ON {}", merge_join, merge_on);
        result.push_join(merge_join);
        result.push_join(SqlExpr::literal("ON ("));
        result.push_join(merge_on);

        // Get ON predicate from entity keys
        let mut predicate_expr = SqlExpr::new();
        let (_field, ancestor_path) = FieldPath::split_basename(root_path.as_str());
        let ancestor_path = ancestor_path.unwrap_or(FieldPath::from(""));
        let mut d = ancestor_path.descendents();

        let columns =
            TreePredicate::columns(entities.get(0).unwrap(), &mut d).map_err(ToqlError::from)?;

        let mut args = Vec::new();
        for e in entities.iter() {
            TreePredicate::args(e, &mut d, &mut args).map_err(ToqlError::from)?;
        }
        let predicate_columns = columns
            .into_iter()
            .map(|c| PredicateColumn::SelfAliased(c))
            .collect::<Vec<_>>();
        predicate_expr.push_predicate(predicate_columns, args);

        let predicate_expr = {
            let merge_resolver = Resolver::new()
                .with_self_alias(&merge_base_alias)
                .with_other_alias(other_alias.as_str());
            merge_resolver
                .resolve(&predicate_expr)
                .map_err(ToqlError::from)?
        };
        result.push_join(SqlExpr::literal(" AND "));
        result.push_join(predicate_expr);
        result.push_join(SqlExpr::literal(")"));

        // Build SQL query statement

        let mut alias_translator = AliasTranslator::new(mysql.alias_format());
        let aux_params = [mysql.aux_params()];
        let aux_params = ParameterMap::new(&aux_params);
        let Sql(sql, args) = result
            .to_sql(&aux_params, &mut alias_translator)
            .map_err(ToqlError::from)?;
        dbg!(&sql);
        dbg!(&args);

        // Load from database
        let args = crate::sql_arg::values_from_ref(&args);
        let query_results = mysql.conn.prep_exec(sql, args)?;

        // Build index
        let mut index: HashMap<u64, Vec<usize>> = HashMap::new();

        let (field, ancestor_path) = FieldPath::split_basename(root_path.as_str());
        let ancestor_path = ancestor_path.unwrap_or(FieldPath::from(""));
        let mut d = ancestor_path.descendents();

        // TODO Batch process rows
        // TODO Introduce traits that do not need copy to vec
        let mut rows = Vec::with_capacity(100);

        for q in query_results {
            rows.push(Row(q?)); // Stream into Vec
        }

        let row_offset = 0; // key must be forst columns in reow
        <T as TreeIndex<Row, ToqlMySqlError>>::index(&mut d, field, &rows, row_offset, &mut index)?;

        // Merge into entities
        for e in entities.iter_mut() {
            <T as TreeMerge<_, ToqlMySqlError>>::merge(
                e,
                &mut d,
                field,
                &rows,
                row_offset,
                &index,
                result.selection_stream(),
            )?;
        }
    }
    Ok(pending_paths)
}

fn load<T, B, C>(
    mysql: &mut MySql<C>,
    query: B,
    page: Option<Page>,
) -> Result<(Vec<T>, Option<(u32, u32)>)>
where
    T: Keyed
        + TreeMap
        + Mapped
        + FromRow<Row,ToqlMySqlError>
        + TreePredicate
        + TreeIndex<Row, ToqlMySqlError>
        + TreeMerge<Row, ToqlMySqlError>,
    B: Borrow<Query<T>>,
    <T as toql::key::Keyed>::Key: FromRow<Row,ToqlMySqlError>,
    C: GenericConnection,
{
    let type_name = <T as Mapped>::type_name();
    if !mysql.cache.registered_roots.read().map_err(ToqlError::from)?.contains(&type_name) {
        let mut cache = &mut *mysql.cache.registry.write().map_err(ToqlError::from)?;
        <T as TreeMap>::map(&mut cache)?;
        mysql.cache.registered_roots.write().map_err(ToqlError::from)?.insert(type_name);
    }

    let (mut entities, mut unmerged_paths, counts) = load_top(mysql, &query, page)?;

    loop {
        let mut pending_paths = load_and_merge(mysql, &query, &mut entities, &unmerged_paths)?;

        // Quit, if all paths have been merged
        if pending_paths.is_empty() {
            break;
        }

        // Select and merge next paths
        unmerged_paths.extend(pending_paths.drain());
    }

    Ok((entities, counts))
}

fn execute_update_delete_sql<C>(statement: Sql, conn: &mut C) -> Result<u64>
where
    C: GenericConnection,
{
    log_sql!(&statement);
    let Sql(update_stmt, params) = statement;

    let mut stmt = conn.prepare(&update_stmt)?;
    let res = stmt.execute(values_from(params))?;
    Ok(res.affected_rows())
}

fn execute_insert_sql<C>(statement: Sql, conn: &mut C) -> Result<u64>
where
    C: GenericConnection,
{
    log_sql!(&statement);
    let Sql(insert_stmt, params) = statement;

    let mut stmt = conn.prepare(&insert_stmt)?;
    let res = stmt.execute(values_from(params))?;
    Ok(res.last_insert_id())
}

pub struct MySql<'a, C: GenericConnection> {
    conn: &'a mut C,
    context : Context,
    cache: &'a Cache
   /*  roles: HashSet<String>,
    registry: &'a SqlMapperRegistry,
    aux_params: HashMap<String, SqlArg>,
    alias_format: AliasFormat, */
}

impl<'a, C: 'a + GenericConnection> MySql<'a, C> {
    /// Create connection wrapper from MySql connection or transaction.
    ///
    /// Use the connection wrapper to access all Toql functionality.
    pub fn from(conn: &'a mut C, cache: &'a Cache) -> MySql<'a, C> {
        Self::with_roles_and_aux_params(conn, cache, HashSet::new(), HashMap::new())
    }

    /// Create connection wrapper from MySql connection or transaction and roles.
    ///
    /// Use the connection wrapper to access all Toql functionality.
    pub fn with_roles(
        conn: &'a mut C,
         cache: &'a Cache,
        roles: HashSet<String>,
    ) -> MySql<'a, C> {
        Self::with_roles_and_aux_params(conn, cache, roles, HashMap::new())
    }
    /// Create connection wrapper from MySql connection or transaction and roles.
    ///
    /// Use the connection wrapper to access all Toql functionality.
    pub fn with_aux_params(
        conn: &'a mut C,
        cache : &'a Cache,
        aux_params: HashMap<String, SqlArg>,
    ) -> MySql<'a, C> {
        Self::with_roles_and_aux_params(conn, cache, HashSet::new(), aux_params)
    }
    /// Create connection wrapper from MySql connection or transaction and roles.
    ///
    /// Use the connection wrapper to access all Toql functionality.
    pub fn with_roles_and_aux_params(
        conn: &'a mut C,
        cache: &'a Cache,
        roles: HashSet<String>,
        aux_params: HashMap<String, SqlArg>,
    ) -> MySql<'a, C> {
        MySql {
            conn,
            cache,
            context: Context {
                roles,
                aux_params,
                alias_format: AliasFormat::Canonical,
            }
        }
    }

    /// Set roles
    ///
    /// After setting the roles all Toql functions are validated against these roles.
    /// Roles on fields can be used to restrict the access (Only super admin can see this field, only group admin can update this field),
    pub fn set_roles(&mut self, roles: HashSet<String>) -> &mut Self {
        self.context.roles = roles;
        self
    }

    pub fn conn(&mut self) -> &'_ mut C {
        self.conn
    }

    pub fn registry(&self) -> std::result::Result<RwLockReadGuard<'_, SqlMapperRegistry>,ToqlError> {
        self.cache.registry.read().map_err(ToqlError::from)
    }
    pub fn roles(&self) -> &HashSet<String> {
        &self.context.roles
    }

    pub fn alias_format(&self) -> AliasFormat {
        self.context.alias_format.to_owned()
    }

    /* pub fn set_aux_params(&mut self, aux_params: HashMap<String, SqlArg>) -> &mut Self {
           self.aux_params = aux_params;
           self
       }
    */
    pub fn aux_params(&self) -> &HashMap<String, SqlArg> {
        &self.context.aux_params
    }

    /// Insert one struct.
    ///
    /// Skip fields in struct that are auto generated with `#[toql(skip_inup)]`.
    /// Returns the last generated id.
    pub fn insert_many<T, Q>(&mut self, paths: Paths<T>, mut entities: &mut [Q]) -> Result<u64>
    where
        T: TreeInsert + Mapped + TreeIdentity,
        Q: BorrowMut<T>,
    {
        use toql::tree::tree_identity::IdentityAction;
        // Build up execution tree
        // Path `a_b_merge1_c_d_merge2_e` becomes
        // [0] = [a, c, e]
        // [1] = [a_b, c_d]
        // [m] = [merge1, merge2]
        // Then execution order is [1], [0], [m]

        // TODO should be possible to impl with &str
        let mut joins: Vec<HashSet<String>> = Vec::new();
        let mut merges: HashSet<String> = HashSet::new();

        toql::backend::insert::plan_insert_order::<T, _>(
            &self.registry()?.mappers,
            &paths.list,
            &mut joins,
            &mut merges,
        )?;

        // Insert root
        let sql = {
            let aux_params = [self.aux_params()];
            let aux_params = ParameterMap::new(&aux_params);
            let home_path = FieldPath::default();

            toql::backend::insert::build_insert_sql::<T, _>(
                &self.registry()?.mappers,
                self.alias_format(),
                &aux_params,
                entities,
                &self.roles(),
                &home_path,
                "",
                "",
            )
        }?;
        if sql.is_none() {
            return Ok(0);
        }
        let sql = sql.unwrap();
        log_sql!(&sql);
        dbg!(sql.to_unsafe_string());
        let Sql(insert_stmt, insert_values) = sql;

        let params = values_from(insert_values);
        {
            let mut stmt = self.conn().prepare(&insert_stmt)?;
            let res = stmt.execute(params)?;
            let affected_rows= res.affected_rows();
            if affected_rows == 0 {
                return Ok(0);
            }
            let home_path = FieldPath::default();
            let mut descendents = home_path.descendents();
                  toql::backend::insert::set_tree_identity(
                    res.last_insert_id(),
                    res.affected_rows(),
                    &mut entities,
                    &mut descendents,
                )?;

          /*   if <T as toql::tree::tree_identity::TreeIdentity>::auto_id() {
                let mut id: u64 = res.last_insert_id() + affected_rows; // first id
                // Build Vec with keys
                let mut ids :Vec<SqlArg> = Vec::with_capacity(affected_rows as usize);
                for _  in 0..affected_rows {
                    ids.push(SqlArg::from(id));
                    id -= 1;
                }

                let home_path = FieldPath::default();
                let mut descendents = home_path.descendents();
                for e in entities.iter_mut() {
                    {
                        let e_mut = e.borrow_mut();
                        <T as toql::tree::tree_identity::TreeIdentity>::set_id(
                            e_mut,
                            &mut descendents,
                            IdentityAction::Set(&mut ids),
                        )?;
                    }
                  
                }
            } */
        }

        // Insert joins
        for l in (0..joins.len()).rev() {
            for p in joins.get(l).unwrap() {
                let mut path = FieldPath::from(&p);

                let sql = {
                    let aux_params = [self.aux_params()];
                    let aux_params = ParameterMap::new(&aux_params);
                    toql::backend::insert::build_insert_sql::<T, _>(
                        &self.registry()?.mappers,
                        self.alias_format(),
                        &aux_params,
                        entities,
                         &self.roles(),
                        &mut path,
                        "",
                        "",
                    )
                }?;
                if sql.is_none() {
                    break;
                }
                let sql = sql.unwrap();
                log_sql!(&sql);
                dbg!(sql.to_unsafe_string());
                let Sql(insert_stmt, insert_values) = sql;

                // Execute
                let params = values_from(insert_values);
                let mut stmt = self.conn().prepare(&insert_stmt)?;
                let res = stmt.execute(params)?;

                // set keys
                let path = FieldPath::from(&p);
                let mut descendents = path.descendents();
                toql::backend::insert::set_tree_identity(
                    res.last_insert_id(),
                    res.affected_rows(),
                    &mut entities,
                    &mut descendents,
                )?;
            }
        }

        // Insert merges
        for p in merges {
            let path = FieldPath::from(&p);

            let sql = {
                let aux_params = [self.aux_params()];
                let aux_params = ParameterMap::new(&aux_params);
                toql::backend::insert::build_insert_sql::<T, _>(
                    &self.registry()?.mappers,
                    self.alias_format(),
                    &aux_params,
                    entities,
                     &self.roles(),
                    &path,
                    "",
                    "",
                )
            }?;
            if sql.is_none() {
                break;
            }
            let sql = sql.unwrap();
            log_sql!(&sql);
            dbg!(sql.to_unsafe_string());
            let Sql(insert_stmt, insert_values) = sql;

            // Execute
            let params = values_from(insert_values);
            let mut stmt = self.conn().prepare(&insert_stmt)?;
            stmt.execute(params)?;

            // Merges must not contain auto value as identity, skip set_tree_identity
        }

        Ok(0)
    }

    pub fn insert_one<T>(&mut self, paths: Paths<T>, entity: &mut T) -> Result<u64>
    where
        T: TreeInsert + Mapped + TreeIdentity,
    {
        self.insert_many::<T, _>(paths, &mut [entity])
    }

    /// Insert one struct.
    ///
    /// Skip fields in struct that are auto generated with `#[toql(skip_inup)]`.
    /// Returns the last generated id.
    pub fn update_many<T, Q>(&mut self, fields: Fields<T>, entities: &mut [Q]) -> Result<()>
    where
        T: TreeUpdate + Mapped + TreeIdentity + TreePredicate + TreeInsert,
        Q: BorrowMut<T>,
    {
        use toql::sql_expr::SqlExpr;
        use toql::tree::tree_identity::IdentityAction;

        // TODO should be possible to impl with &str
        let mut joins: HashMap<String, HashSet<String>> = HashMap::new();
        let mut merges: HashMap<String, HashSet<String>> = HashMap::new();

        //  toql::backend::insert::split_basename(&fields.list, &mut path_fields, &mut paths);
        
        toql::backend::update::plan_update_order::<T, _>(
            &self.registry()?.mappers,
            &fields.list,
            &mut joins,
            &mut merges,
        )?;

        for (path, fields) in joins {
            let sqls = {
                let field_path = FieldPath::from(&path);
                toql::backend::update::build_update_sql::<T, _>(
                    self.alias_format(),
                    entities,
                    &field_path,
                    &fields,
                    self.roles(),
                    "",
                    "",
                )
            }?;

            // Update joins
            for sql in sqls {
                dbg!(sql.to_unsafe_string());
                execute_update_delete_sql(sql, self.conn)?;
            }
        }

        // Delete existing merges and insert new merges

        for (path, fields) in merges {
            // Build delete sql

            let parent_path = FieldPath::from(&path);
            let entity = entities.get(0).unwrap().borrow();
            let columns = <T as TreePredicate>::columns(entity, &mut parent_path.descendents())?;
            let mut args = Vec::new();
            for e in entities.iter() {
                <T as TreePredicate>::args(e.borrow(), &mut parent_path.descendents(), &mut args)?;
            }
            let columns = columns
                .into_iter()
                .map(|c| PredicateColumn::SelfAliased(c))
                .collect::<Vec<_>>();

            // Construct sql
            let mut key_predicate: SqlExpr = SqlExpr::new();
            key_predicate.push_predicate(columns, args);

            for merge in fields {
               let merge_path = FieldPath::from(&merge);
                let sql = {
                    
                    let type_name = <T as Mapped>::type_name();
                    let registry = &*self.registry()?;
                    let mut sql_builder = SqlBuilder::new(&type_name, registry);
                    let delete_expr =
                        sql_builder.build_merge_delete(&merge_path, key_predicate.to_owned())?;

                    let mut alias_translator = AliasTranslator::new(self.alias_format());
                    let resolver = Resolver::new();
                        resolver
                        .to_sql(&delete_expr, &mut alias_translator)
                        .map_err(ToqlError::from)?
                };

                dbg!(sql.to_unsafe_string());
                execute_update_delete_sql(sql, self.conn)?;

                // Update association keys
                for e in entities.iter_mut() {
                    let mut descendents = parent_path.descendents();
                    <T as TreeIdentity>::set_id(
                        e.borrow_mut(),
                        &mut descendents,
                       &IdentityAction::Refresh,
                    )?;
                }

                // Insert
                let aux_params = [self.aux_params()];
                let aux_params = ParameterMap::new(&aux_params);
                let sql = toql::backend::insert::build_insert_sql(
                    &self.registry()?.mappers,
                    self.alias_format(),
                    &aux_params,
                    entities,
                     &self.roles(),
                    &merge_path,
                    "",
                    "",
                )?;
                if let Some(sql) = sql {
                    dbg!(sql.to_unsafe_string());
                    execute_update_delete_sql(sql, self.conn)?;
                }
            }
        }

        Ok(())
    }

    /// Delete a struct.
    ///
    /// The field that is used as key must be attributed with `#[toql(delup_key)]`.
    /// Returns the number of deleted rows.
    /// pub fn select_one<K>(&mut self, key: K) -> Result<<K as Key>::Entity>

    pub fn delete_one<K>(&mut self, key: K) -> Result<u64>
    where
        K: Key + Into<Query<<K as Key>::Entity>>,
        <K as Key>::Entity: Mapped + TreeMap,
    {
        /*  let sql_mapper = self.registry.mappers.get( &<K as Key>::Entity::type_name() )
        .ok_or( ToqlError::MapperMissing(<K as Key>::Entity::type_name()))?; */

        let query = Query::from(key);

        self.delete_many(query)

        //execute_update_delete_sql(sql, self.conn)
    }

    /// Delete a collection of structs.
    ///
    /// The field that is used as key must be attributed with `#[toql(delup_key)]`.
    /// Returns the number of deleted rows.
    pub fn delete_many<T, B>(&mut self, query: B) -> Result<u64>
    where
        T: Mapped + TreeMap,
        B: Borrow<Query<T>>,
    {
       
        let type_name = <T as Mapped>::type_name();
        if !self.cache.registered_roots.read().map_err(ToqlError::from)?.contains(&type_name) {
            let mut cache = &mut *self.cache.registry.write().map_err(ToqlError::from)?;
            <T as TreeMap>::map(&mut cache)?;

            self.cache.registered_roots.write().map_err(ToqlError::from)?.insert(type_name);
        }

        let result = SqlBuilder::new(&<T as Mapped>::type_name(), &*self.cache.registry.read().map_err(ToqlError::from)?)
            .with_aux_params(self.aux_params().clone()) // todo ref
            .with_roles(self.roles().clone()) // todo ref
            .build_delete(query.borrow())?;

        // No arguments, nothing to delete
        if result.is_empty() {
            Ok(0)
        } else {
            let pa = [&self.context.aux_params];
            let p = ParameterMap::new(&pa);
            let mut alias_translator = AliasTranslator::new(self.alias_format());
            let sql = result
                .to_sql(&p, &mut alias_translator)
                .map_err(ToqlError::from)?;
            execute_update_delete_sql(sql, self.conn)
        }
    }
   
    /// Update a single struct.
    ///
    /// Optional fields with value `None` are not updated. See guide for details.
    /// The field that is used as key must be attributed with `#[toql(key)]`.
    /// Returns the number of updated rows.
    ///

    pub fn update_one<T>(&mut self, fields: Fields<T>, entity: &mut T) -> Result<()>
    where
        T: TreeUpdate + Mapped + TreeIdentity + TreePredicate + TreeInsert,
    {
        self.update_many::<T, _>(fields, &mut [entity])
    }

    /// Counts the number of rows that match the query predicate.
    ///
    /// Returns a struct or a [ToqlMySqlError](../toql/error/enum.ToqlMySqlError.html) if no struct was found _NotFound_ or more than one _NotUnique_.
    pub fn count<T, B>(&mut self, query: B) -> Result<u64>
    where
        T: toql::key::Keyed + toql::sql_mapper::mapped::Mapped,
        B: Borrow<Query<T>>,
    {
        /* let sql_mapper = self
        .registry
        .mappers
        .get(&<T as Mapped>::type_name())
        .ok_or(ToqlError::MapperMissing(<T as Mapped>::type_name()))?; */

        let mut alias_translator = AliasTranslator::new(self.alias_format());

        let result = SqlBuilder::new(&<T as Mapped>::type_name(), &*self.cache.registry.read().map_err(ToqlError::from)?)
            .with_roles(self.roles().clone())
            .with_aux_params(self.aux_params().clone())
            .build_count("", query.borrow(), false)?;
        let p = [self.aux_params()];
        let aux_params = ParameterMap::new(&p);

        let sql = result
            .to_sql(&aux_params, &mut alias_translator)
            .map_err(ToqlError::from)?;

        log_sql!(sql);
        let result = self.conn.prep_exec(&sql.0, values_from_ref(&sql.1))?;

        let count = result.into_iter().next().unwrap().unwrap().get(0).unwrap();

        Ok(count)
    }

    /// Load a struct with dependencies for a given Toql query.
    ///
    /// Returns a struct or a [ToqlMySqlError](../toql/error/enum.ToqlMySqlError.html) if no struct was found _NotFound_ or more than one _NotUnique_.
    pub fn load_one<T, B>(&mut self, query: B) -> Result<T>
    where
        T: Keyed
            + Mapped
            + TreeMap
            + FromRow<Row,ToqlMySqlError>
            + TreePredicate
            + TreeIndex<Row, ToqlMySqlError>
            + TreeMerge<Row, ToqlMySqlError>,
        B: Borrow<Query<T>>,
        <T as Keyed>::Key: FromRow<Row,ToqlMySqlError>,
    {
        // <Self as Load<T>>::load_one(self, query.borrow())
        let (mut e, _) = load(self, query.borrow(), Some(Page::Uncounted(0, 2)))?;
        match e.len() {
            0 => Err(ToqlError::NotFound.into()),
            1 => Ok(e.pop().unwrap()),
            _ => Err(ToqlError::NotUnique.into()),
        }
    }

    /// Load a vector of structs with dependencies for a given Toql query.
    ///
    /// Returns a tuple with the structs and an optional tuple of count values.
    /// If `count` argument is `false`, no count queries are run and the resulting `Option<(u32,u32)>` will be `None`
    /// otherwise the count queries are run and it will be `Some((total count, filtered count))`.
    pub fn load_many<T, B>(&mut self, query: B) -> Result<Vec<T>>
    where
        T: Keyed
            + Mapped
            + TreeMap
            + FromRow<Row,ToqlMySqlError>
            + TreePredicate
            + TreeKeys
            + TreeIndex<Row, ToqlMySqlError>
            + TreeMerge<Row, ToqlMySqlError>,
        B: Borrow<Query<T>>,
        <T as Keyed>::Key: FromRow<Row,ToqlMySqlError>,
    {
        let (entities, _) = load(self, query, None)?;
        Ok(entities)
    }

    /// Load a vector of structs with dependencies for a given Toql query.
    ///
    /// Returns a tuple with the structs and an optional tuple of count values.
    /// If `count` argument is `false`, no count queries are run and the resulting `Option<(u32,u32)>` will be `None`
    /// otherwise the count queries are run and it will be `Some((unpaged count, unfiltered count))`.
    pub fn load_page<T, B>(&mut self, query: B, page: Page) -> Result<(Vec<T>, Option<(u32, u32)>)>
    where
        T: Keyed
        + TreeMap
            + Mapped
            + FromRow<Row,ToqlMySqlError>
            + TreePredicate
            + TreeIndex<Row, ToqlMySqlError>
            + TreeMerge<Row, ToqlMySqlError>,
        B: Borrow<Query<T>>,
        <T as Keyed>::Key: FromRow<Row,ToqlMySqlError>,
    {
        let entities_page = load(self, query.borrow(), Some(page))?;

        Ok(entities_page)
    }
}
