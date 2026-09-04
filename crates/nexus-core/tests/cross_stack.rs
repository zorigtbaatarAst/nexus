//! The claim this milestone exists to make: a change to a backend service method reaches
//! the frontend components that render it, through the GraphQL contract.
//!
//! Built as a real project on disk rather than a mocked graph, because the interesting
//! failures live in extraction and resolution, not in the traversal.

use nexus_core::impact::{Direction, ImpactQuery};
use nexus_core::report::Resolved;
use nexus_core::Engine;
use std::fs;
use std::path::{Path, PathBuf};

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, body).expect("write");
}

fn fixture(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("nexus-xstack-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("mkdir");

    write(
        &root,
        "build.gradle",
        "dependencies { implementation 'org.springframework.boot:spring-boot-starter:3.5.0' }\n",
    );

    write(
        &root,
        "backend/src/main/java/mn/sales/vehicle/VehicleService.java",
        r#"
package mn.sales.vehicle;

public class VehicleService {
    private final VehicleRepository repo;

    public AntPage<VehicleDto> list(AntPageable pagination) {
        return repo.findAll(pagination);
    }
}
"#,
    );

    write(
        &root,
        "backend/src/main/java/mn/sales/vehicle/VehicleGraphQLController.java",
        r#"
package mn.sales.vehicle;

@Controller
public class VehicleGraphQLController {
    private final VehicleService vehicleService;

    @QueryMapping
    public AntPage<VehicleDto> vehicles(@Argument AntPageable pagination) {
        return vehicleService.list(pagination);
    }
}
"#,
    );

    write(
        &root,
        "frontend/src/lib/graphql/vehicle.ts",
        r#"
import { gql } from '@apollo/client';

export const Vehicles = gql`
  query Vehicles($pagination: AntPageable) {
    vehicles(pagination: $pagination) { id plate }
  }
`;
"#,
    );

    write(
        &root,
        "frontend/src/app/vehicles/page.tsx",
        r#"
import { useQuery } from '@apollo/client/react';
import { VehiclesDocument } from '@/types/graphql-generated';

export const VehiclesPage = () => {
  const { data } = useQuery(VehiclesDocument);
  return <div>{data?.vehicles?.length}</div>;
};
"#,
    );

    // Generated output must not be indexed: symbols nobody wrote and nobody can change.
    write(
        &root,
        "frontend/src/types/graphql-generated.ts",
        "export const VehiclesDocument = {} as any;\n",
    );
    root
}

fn scan(root: &Path) -> Engine {
    let (mut engine, _) = Engine::init(root, nexus_lang_pack::default_registry()).expect("init");
    let report = engine.scan().expect("scan");
    assert_eq!(
        report.files_failed, 0,
        "no file should fail to parse: {:?}",
        report.warnings
    );
    engine
}

#[test]
fn a_backend_change_reaches_the_frontend_through_the_graphql_seam() {
    let root = fixture("seam");
    let engine = scan(&root);

    let q = ImpactQuery {
        target: "mn.sales.vehicle.VehicleService#list".into(),
        direction: Direction::Reverse,
        max_depth: 6,
        ..Default::default()
    };
    let Resolved::One(report) = engine.impact(&q).expect("impact") else {
        panic!("the target should be unambiguous");
    };

    let reached: Vec<&str> = report.items.iter().map(|i| i.fqn.as_str()).collect();

    for expected in [
        "mn.sales.vehicle.VehicleGraphQLController#vehicles(AntPageable)",
        // Namespaced by module: the fixture's backend is `backend/`, and six services in
        // one repository cannot all own `Query.vehicles`.
        "graphql:backend:Query.vehicles",
        "graphql:op:Vehicles",
        "frontend/src/app/vehicles/page#VehiclesPage",
    ] {
        assert!(
            reached.contains(&expected),
            "missing {expected} in {reached:?}"
        );
    }

    assert!(
        report.crossed_seam > 0,
        "the trace must be recorded as crossing the seam"
    );

    // Every hop is explainable: the component's path names the whole chain, so a human
    // can argue with the conclusion rather than merely believe it.
    let component = report
        .items
        .iter()
        .find(|i| i.fqn.ends_with("#VehiclesPage"))
        .expect("the component");
    let edges: Vec<&str> = component.path.iter().map(|h| h.edge).collect();
    assert_eq!(
        edges,
        vec!["calls", "routes", "calls_graphql", "calls_graphql"],
        "{:?}",
        component.path
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn generated_files_are_not_indexed() {
    let root = fixture("generated");
    let engine = scan(&root);
    let status = engine.status().expect("status");
    assert!(status.files > 0);

    let q = ImpactQuery {
        target: "graphql-generated".into(),
        ..Default::default()
    };
    assert!(
        matches!(
            engine.impact(&q).expect("impact"),
            Resolved::NotFound { .. }
        ),
        "codegen output is not source and must stay out of the graph"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn an_ambiguous_target_returns_candidates_rather_than_a_guess() {
    let root = fixture("ambiguous");
    let engine = scan(&root);

    // `list` matches a method here; `vehicles` matches both the schema coordinate and the
    // controller method, which is exactly the case that must not be silently resolved.
    let q = ImpactQuery {
        target: "vehicles".into(),
        ..Default::default()
    };
    match engine.impact(&q).expect("impact") {
        Resolved::Ambiguous(candidates) => {
            assert!(candidates.len() > 1, "{candidates:?}");
        }
        other => panic!("expected candidates, got {other:?}"),
    }
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_forward_trace_from_a_schema_field_reaches_the_repository() {
    let root = fixture("forward");
    let engine = scan(&root);

    let q = ImpactQuery {
        target: "graphql:Query.vehicles".into(),
        direction: Direction::Forward,
        max_depth: 6,
        ..Default::default()
    };
    let Resolved::One(report) = engine.impact(&q).expect("impact") else {
        panic!("unambiguous");
    };
    let reached: Vec<&str> = report.items.iter().map(|i| i.fqn.as_str()).collect();
    assert!(
        reached.iter().any(|f| f.contains("VehicleService#list")),
        "forward from a schema field should reach the service: {reached:?}"
    );
    let _ = fs::remove_dir_all(&root);
}

/// A project where both halves of the seam name the same coordinate.
///
/// `.graphqls` declares `Query.vehicles`; a `@QueryMapping` handler implements it. Both
/// analyzers emit a symbol at that FQN, because it is the join key the frontend points at,
/// and `symbols` is unique on FQN — so exactly one of them owns the row.
fn seam_fixture(name: &str, schema: bool) -> PathBuf {
    let root = std::env::temp_dir().join(format!("nexus-own-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("mkdir");

    if schema {
        write(
            &root,
            "backend/src/main/resources/graphql/schema.graphqls",
            "type Query {\n  vehicles(pagination: AntPageable): AntPage!\n}\n",
        );
    }
    write(
        &root,
        "backend/src/main/java/mn/sales/VehicleController.java",
        r#"
package mn.sales;

@Controller
public class VehicleController {
    @QueryMapping
    public AntPage<VehicleDto> vehicles(@Argument AntPageable pagination) {
        return null;
    }
}
"#,
    );
    root
}

const COORDINATE: &str = "graphql:backend:Query.vehicles";

fn coordinate(engine: &Engine) -> nexus_core::report::SymbolDetail {
    match engine.symbol(COORDINATE).expect("symbol") {
        Resolved::One(detail) => detail,
        other => panic!("the coordinate should resolve to exactly one symbol: {other:?}"),
    }
}

fn owning_file(engine: &Engine) -> String {
    coordinate(engine).file
}

#[test]
fn a_schema_declaration_survives_a_rescan_of_only_the_resolver() {
    let root = seam_fixture("partial", true);
    let (mut engine, _) = Engine::init(&root, nexus_lang_pack::default_registry()).expect("init");
    engine.scan().expect("scan");

    assert!(
        owning_file(&engine).ends_with(".graphqls"),
        "the schema declares the coordinate, so the schema owns the row (ADR-014)"
    );

    // Edit only the Java half. The schema file is untouched, so a rescan does not re-parse
    // it — and the resolver's symbols alone must not take the coordinate away from it.
    let handler = root.join("backend/src/main/java/mn/sales/VehicleController.java");
    let edited = fs::read_to_string(&handler)
        .expect("read")
        .replace("    @QueryMapping", "    @QueryMapping\n    @Transactional");
    fs::write(&handler, edited).expect("write");

    let report = engine.rescan().expect("rescan");
    let touched: Vec<_> = report
        .items
        .iter()
        .filter(|i| i.entity == "symbol" && i.fqn.as_deref() == Some(COORDINATE))
        .collect();
    assert!(
        touched.is_empty(),
        "nothing about the schema field changed, and a file that was not re-parsed cannot \
         have declared it anew: {touched:?}"
    );

    let after = coordinate(&engine);
    assert!(
        after.file.ends_with(".graphqls"),
        "the resolver took the coordinate away from the file that declares it"
    );
    // The declaration itself, not merely which file holds it: the row's signature is read
    // back off the file it points at, so the schema's line is what a reader gets.
    assert!(
        after
            .source
            .as_deref()
            .is_some_and(|src| src.contains("vehicles(pagination: AntPageable): AntPage!")),
        "the stored declaration should still be the schema's: {:?}",
        after.source
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn rescanning_the_resolver_does_not_multiply_its_route_edge() {
    // `replace_edges_for_file` deleted by the file that owns an edge's *source symbol*.
    // Once the schema owns the coordinate that stopped being the file that produced the
    // edge, so a rescan of the resolver deleted nothing and inserted another copy: one
    // edge became two, then three, on every rescan of a file nobody had touched.
    let root = seam_fixture("edges", true);
    let (mut engine, _) = Engine::init(&root, nexus_lang_pack::default_registry()).expect("init");
    engine.scan().expect("scan");

    let routes = |e: &Engine| -> usize {
        coordinate(e)
            .depends_on
            .iter()
            .filter(|n| n.edge == "routes")
            .count()
    };
    let first = routes(&engine);
    assert_eq!(first, 1, "one handler, one route edge");

    let handler = root.join("backend/src/main/java/mn/sales/VehicleController.java");
    for i in 0..3 {
        let edited = format!(
            "{}\n// touch {i}\n",
            fs::read_to_string(&handler).expect("read")
        );
        fs::write(&handler, edited).expect("write");
        engine.rescan().expect("rescan");
        assert_eq!(
            routes(&engine),
            first,
            "rescan {i} multiplied an edge instead of replacing it"
        );
    }

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn deleting_the_schema_hands_the_coordinate_back_to_the_resolver() {
    // Yielding must be reversible. The declaration's row is soft-deleted with its file, and
    // the resolver — untouched, so never re-parsed on its own account — has to be brought
    // back in to stand in for it, or the coordinate stays buried until a full scan.
    let root = seam_fixture("deleted", true);
    let (mut engine, _) = Engine::init(&root, nexus_lang_pack::default_registry()).expect("init");
    engine.scan().expect("scan");
    assert!(owning_file(&engine).ends_with(".graphqls"));

    fs::remove_file(root.join("backend/src/main/resources/graphql/schema.graphqls")).expect("rm");
    engine.rescan().expect("rescan");

    let after = coordinate(&engine);
    assert!(
        after.file.ends_with(".java"),
        "with the declaration gone the resolver's own route stands: {}",
        after.file
    );
    assert!(
        after.depends_on.iter().any(|n| n.edge == "routes"),
        "and it keeps the routes edge to the handler: {:?}",
        after.depends_on
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_resolver_without_a_schema_still_gets_its_route() {
    // The other half of the rule: yielding to a declaration must not mean emitting nothing.
    // A project that generates its schema at build time has only the handler, and the route
    // symbol is what the frontend's operation resolves against.
    let root = seam_fixture("schemaless", false);
    let (mut engine, _) = Engine::init(&root, nexus_lang_pack::default_registry()).expect("init");
    engine.scan().expect("scan");

    let file = owning_file(&engine);
    assert!(
        file.ends_with(".java"),
        "with nothing declaring the coordinate, the resolver's own route symbol stands: {file}"
    );

    let q = ImpactQuery {
        target: COORDINATE.into(),
        direction: Direction::Forward,
        max_depth: 2,
        ..Default::default()
    };
    let Resolved::One(report) = engine.impact(&q).expect("impact") else {
        panic!("the coordinate should be unambiguous");
    };
    assert!(
        report
            .items
            .iter()
            .any(|i| i.fqn.contains("VehicleController#vehicles")),
        "the routes edge from coordinate to handler must survive: {:?}",
        report.items.iter().map(|i| &i.fqn).collect::<Vec<_>>()
    );

    let _ = fs::remove_dir_all(&root);
}
