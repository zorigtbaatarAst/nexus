package mn.pay;

import java.util.Optional;
import org.springframework.data.jpa.repository.JpaRepository;

public interface PaymentRepository extends JpaRepository<Payment, String> {

    boolean existsByIdempotencyKey(String idempotencyKey);

    Optional<Payment> findByIdempotencyKey(String idempotencyKey);
}
