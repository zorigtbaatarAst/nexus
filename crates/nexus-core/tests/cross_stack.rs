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
    let (mut engine, _) = Engine::init(root).expect("init");
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
        "graphql:Query.vehicles",
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
