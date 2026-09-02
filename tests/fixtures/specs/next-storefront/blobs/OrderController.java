package mn.shop.api;

import java.util.List;
import org.springframework.graphql.data.method.annotation.Argument;
import org.springframework.graphql.data.method.annotation.QueryMapping;
import org.springframework.stereotype.Controller;

@Controller
public class OrderController {

    private final OrderRepository repository;

    public OrderController(OrderRepository repository) {
        this.repository = repository;
    }

    @QueryMapping
    public List<OrderDto> orders() {
        return repository.findAll().stream().map(OrderDto::of).toList();
    }

    @QueryMapping
    public OrderDto order(@Argument String id) {
        return repository.findById(id).map(OrderDto::of).orElse(null);
    }
}
