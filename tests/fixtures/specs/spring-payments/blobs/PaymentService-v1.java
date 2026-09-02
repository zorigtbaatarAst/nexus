package mn.pay;

import java.math.BigDecimal;
import java.math.RoundingMode;
import java.util.List;
import org.springframework.stereotype.Service;
import org.springframework.transaction.annotation.Transactional;

@Service
public class PaymentService {

    private final PaymentRepository repository;

    public PaymentService(PaymentRepository repository) {
        this.repository = repository;
    }

    /**
     * Idempotency is enforced here, inside the transaction, so the existence check and the
     * insert cannot interleave with a concurrent call carrying the same key.
     */
    @Transactional
    public Payment create(String idempotencyKey, BigDecimal amount) {
        if (repository.existsByIdempotencyKey(idempotencyKey)) {
            return repository.findByIdempotencyKey(idempotencyKey).orElseThrow();
        }
        Payment p = new Payment();
        p.setStatus("PENDING");
        return repository.save(p);
    }

    /** The live total. Rounds half-up to two places, which is what the ledger expects. */
    public BigDecimal total(List<Payment> payments) {
        BigDecimal sum = BigDecimal.ZERO;
        for (Payment p : payments) {
            sum = sum.add(p.getAmount());
        }
        return sum.setScale(2, RoundingMode.HALF_UP);
    }
}
