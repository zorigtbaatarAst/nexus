package mn.acme.common;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.io.IOException;
import java.math.BigDecimal;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import java.util.stream.Stream;
import org.junit.jupiter.api.Test;

/**
 * L1 for A2-shared-type-change.
 *
 * <p>The task: "Money should carry a scale of 4 rather than 2, for FX-denominated orders."
 *
 * <p>The observable behaviour is one sentence: <em>an amount with four decimal places survives a
 * trip through Money, and one with fewer is carried at the new scale</em>. Both assertions run
 * the type rather than read it, so nothing here cares whether the scale became a constant, a
 * parameter or a different rounding mode — only that a fourth decimal place is no longer thrown
 * away. {@code setScale(4, HALF_UP)}, {@code setScale(4, HALF_EVEN)} and a configurable scale
 * defaulting to 4 all pass; deleting the rounding altogether does not, because an amount would
 * then carry whatever scale its caller happened to supply rather than the scale the task asks
 * for.
 *
 * <p>The bound is "at least four", not "exactly four", for the same reason A1's is "at least
 * 128": the task asks for four places of precision and a fix that gives more has still given
 * four. An equality would grade the reference fix.
 *
 * <p>The third test is a guard and passes vacuously at the start commit — neither service
 * mentions a scale there, and neither needs to change for a correct fix. It exists to catch the
 * shape where the shared type is widened and a consumer quietly rounds the precision straight
 * back off. That means L1 for this task is, honestly, a one-module gate: {@code required_sites}
 * is the separate check that the agent looked at the two services, and an L3 failure beside an
 * L1 pass is information about the agent's search rather than a defect here.
 */
class HiddenTest {

    /** An FX rate needs four places; two is what the old scale left. */
    private static final BigDecimal FX_AMOUNT = new BigDecimal("1.2345");

    /** A rounding call that pins a scale in source: `setScale(2, ...)` or `scale = 2`. */
    private static final Pattern PINNED_SCALE =
            Pattern.compile("(?:setScale\\s*\\(\\s*|(?i:scale)\\s*=\\s*)(\\d+)");

    private static String stripJavaComments(String source) {
        return source.replaceAll("(?s)/\\*.*?\\*/", " ").replaceAll("(?m)//[^\\n]*", " ");
    }

    /** The repository root: the nearest ancestor holding the Gradle settings file. */
    private static Path repoRoot() {
        Path candidate = Path.of("").toAbsolutePath();
        while (candidate != null) {
            if (Files.isRegularFile(candidate.resolve("settings.gradle"))) {
                return candidate;
            }
            candidate = candidate.getParent();
        }
        throw new IllegalStateException(
                "no ancestor of " + Path.of("").toAbsolutePath() + " holds settings.gradle");
    }

    @Test
    void anFxAmountKeepsItsFourthDecimalPlace() {
        BigDecimal carried = new Money(FX_AMOUNT, "MNT").amount();
        assertEquals(
                0,
                carried.compareTo(FX_AMOUNT),
                "Money rounded " + FX_AMOUNT + " to " + carried + "; an FX-denominated amount loses "
                        + "precision the moment it is constructed");
    }

    @Test
    void anAmountWithFewerPlacesIsCarriedAtTheNewScale() {
        BigDecimal carried = new Money(new BigDecimal("1.5"), "MNT").amount();
        assertTrue(
                carried.scale() >= 4,
                "Money carries a scale of " + carried.scale() + ", not the four places FX orders need");
    }

    @Test
    void noServiceRoundsThePrecisionStraightBackOff() throws IOException {
        List<String> narrowing = new ArrayList<>();
        Path services = repoRoot().resolve("services");
        if (!Files.isDirectory(services)) {
            return; // nothing consumes the shared type here, so nothing can narrow it back
        }

        try (Stream<Path> tree = Files.walk(services)) {
            for (Path file : tree.filter(Files::isRegularFile).sorted().toList()) {
                // Production sources only: a service's own tests may legitimately talk about
                // two-place money without the service rounding to it.
                if (!file.toString().endsWith(".java") || !file.toString().contains("src/main/java")) {
                    continue;
                }
                Matcher pinned = PINNED_SCALE.matcher(stripJavaComments(Files.readString(file)));
                while (pinned.find()) {
                    if (Integer.parseInt(pinned.group(1)) < 4) {
                        narrowing.add(repoRoot().relativize(file) + " pins scale " + pinned.group(1));
                    }
                }
            }
        }

        assertTrue(
                narrowing.isEmpty(),
                "a service rounds money back below four places, undoing the shared type's new "
                        + "scale: " + narrowing);
    }
}
